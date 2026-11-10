//! A `GtkMediaStream` on top of the classic `playbin`, used for audio and video.
//!
//! GTK's own GStreamer backend goes through GstPlay/playbin3, which on
//! GStreamer 1.28 aborts the whole process on ordinary ID3-tagged MP3s and
//! blocks the main thread forever when a file without a usable decoder is
//! dropped. playbin has neither problem. Video frames come out of an appsink
//! as plain BGRx textures shown in a `gtk::Picture`.

use gst::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, prelude::*};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod imp {
    use super::*;
    use std::cell::{Cell, OnceCell, RefCell};

    #[derive(Default)]
    pub struct PlayStream {
        pub playbin: OnceCell<gst::Element>,
        pub has_video: Cell<bool>,
        pub bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
        pub tick: RefCell<Option<glib::SourceId>>,
        pub seeking: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayStream {
        const NAME: &'static str = "ChoothoPlayStream";
        type Type = super::PlayStream;
        type ParentType = gtk::MediaStream;
    }

    impl ObjectImpl for PlayStream {
        fn dispose(&self) {
            self.stop_tick();
            self.bus_watch.take();
            if let Some(p) = self.playbin.get() {
                let _ = p.set_state(gst::State::Null);
            }
        }
    }

    impl MediaStreamImpl for PlayStream {
        fn play(&self) -> bool {
            if self.obj().is_ended() {
                self.seek(0);
            }
            if self.playbin().set_state(gst::State::Playing).is_err() {
                return false;
            }
            self.start_tick();
            true
        }

        fn pause(&self) {
            let _ = self.playbin().set_state(gst::State::Paused);
            self.stop_tick();
            self.report_position();
        }

        fn seek(&self, timestamp: i64) {
            let pos = gst::ClockTime::from_useconds(timestamp.max(0) as u64);
            if self.playbin().seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE, pos).is_ok() {
                self.seeking.set(true);
            } else {
                self.obj().seek_failed();
            }
        }

        fn update_audio(&self, muted: bool, volume: f64) {
            self.playbin().set_property("mute", muted);
            self.playbin().set_property("volume", volume);
        }
    }

    impl PlayStream {
        pub fn playbin(&self) -> &gst::Element {
            self.playbin.get().expect("playbin is set in new()")
        }

        fn start_tick(&self) {
            self.stop_tick();
            let weak = self.obj().downgrade();
            let id = glib::timeout_add_local(Duration::from_millis(250), move || {
                let Some(obj) = weak.upgrade() else { return glib::ControlFlow::Break };
                obj.imp().report_position();
                glib::ControlFlow::Continue
            });
            self.tick.replace(Some(id));
        }

        fn stop_tick(&self) {
            if let Some(id) = self.tick.take() {
                id.remove();
            }
        }

        pub fn report_position(&self) {
            if let Some(pos) = self.playbin().query_position::<gst::ClockTime>() {
                self.obj().update(pos.useconds() as i64);
            }
        }

        pub fn on_message(&self, msg: &gst::Message) {
            let obj = self.obj();
            match msg.view() {
                gst::MessageView::AsyncDone(_) => {
                    if !obj.is_prepared() {
                        let duration = self.playbin().query_duration::<gst::ClockTime>();
                        let duration = duration.map_or(0, |d| d.useconds() as i64);
                        obj.stream_prepared(true, self.has_video.get(), true, duration);
                    }
                    if self.seeking.replace(false) {
                        obj.seek_success();
                        self.report_position();
                    }
                }
                gst::MessageView::Eos(_) if obj.is_loop() => self.seek(0),
                gst::MessageView::Eos(_) => {
                    self.stop_tick();
                    self.report_position();
                    obj.stream_ended();
                }
                gst::MessageView::Error(e) => {
                    self.stop_tick();
                    let _ = self.playbin().set_state(gst::State::Null);
                    obj.set_error(e.error());
                }
                _ => {}
            }
        }
    }
}

glib::wrapper! {
    pub struct PlayStream(ObjectSubclass<imp::PlayStream>)
        @extends gtk::MediaStream,
        @implements gdk::Paintable;
}

impl PlayStream {
    /// Audio only: any video stream (cover art in some containers) is discarded.
    pub fn audio(path: &Path) -> Self {
        let sink = gst::ElementFactory::make("fakesink").build().ok();
        Self::new(path, sink, false)
    }

    /// Plays `path`, drawing its frames into `picture`.
    pub fn video(path: &Path, picture: &gtk::Picture) -> Self {
        Self::new(path, Some(frame_sink(picture).upcast()), true)
    }

    fn new(path: &Path, video_sink: Option<gst::Element>, has_video: bool) -> Self {
        let obj: Self = glib::Object::new();
        let imp = obj.imp();
        imp.has_video.set(has_video);
        let uri = glib::filename_to_uri(path, None).map(|u| u.to_string()).unwrap_or_default();
        let playbin = match gst::ElementFactory::make("playbin").property("uri", uri).build() {
            Ok(p) => {
                if let Some(sink) = video_sink {
                    p.set_property("video-sink", &sink);
                }
                p
            }
            Err(_) => {
                obj.set_error(glib::Error::new(gst::CoreError::MissingPlugin, "GStreamer playbin is missing"));
                gst::Pipeline::new().upcast()
            }
        };
        imp.playbin.set(playbin.clone()).unwrap();

        if let Some(bus) = playbin.bus() {
            let weak = obj.downgrade();
            let guard = bus.add_watch_local(move |_, msg| {
                if let Some(obj) = weak.upgrade() {
                    obj.imp().on_message(msg);
                }
                glib::ControlFlow::Continue
            });
            imp.bus_watch.replace(guard.ok());
        }
        // Preroll: the duration and the first video frame are ready before play.
        let _ = playbin.set_state(gst::State::Paused);
        obj
    }
}

struct Frame {
    data: glib::Bytes,
    width: i32,
    height: i32,
    stride: usize,
}

/// An appsink that hands the newest frame to `picture` on the main thread,
/// dropping frames the UI had no time to show.
fn frame_sink(picture: &gtk::Picture) -> gst_app::AppSink {
    let caps = gst_video::VideoCapsBuilder::new().format(gst_video::VideoFormat::Bgrx).build();
    let sink = gst_app::AppSink::builder().caps(&caps).max_buffers(1).drop(true).build();

    let latest: Arc<Mutex<Option<Frame>>> = Arc::default();
    let queued = Arc::new(AtomicBool::new(false));
    let target = glib::SendWeakRef::from(picture.downgrade());

    let push = move |sink: &gst_app::AppSink, preroll: bool| -> Result<gst::FlowSuccess, gst::FlowError> {
        let sample = if preroll { sink.pull_preroll() } else { sink.pull_sample() }.map_err(|_| gst::FlowError::Eos)?;
        let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else { return Ok(gst::FlowSuccess::Ok) };
        let info = gst_video::VideoInfo::from_caps(caps).map_err(|_| gst::FlowError::NotNegotiated)?;
        let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).map_err(|_| gst::FlowError::Error)?;
        let data = glib::Bytes::from(frame.plane_data(0).map_err(|_| gst::FlowError::Error)?);
        let frame = Frame { data, width: info.width() as i32, height: info.height() as i32, stride: info.stride()[0] as usize };
        *latest.lock().unwrap() = Some(frame);

        if !queued.swap(true, Ordering::AcqRel) {
            let (latest, queued, target) = (latest.clone(), queued.clone(), target.clone());
            glib::idle_add_once(move || {
                queued.store(false, Ordering::Release);
                let (Some(pic), Some(f)) = (target.upgrade(), latest.lock().unwrap().take()) else { return };
                let tex = gdk::MemoryTexture::new(f.width, f.height, gdk::MemoryFormat::B8g8r8x8, &f.data, f.stride);
                pic.set_paintable(Some(&tex));
            });
        }
        Ok(gst::FlowSuccess::Ok)
    };
    let push_preroll = push.clone();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |s| push(s, false))
            .new_preroll(move |s| push_preroll(s, true))
            .build(),
    );
    sink
}
