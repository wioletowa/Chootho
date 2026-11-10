//! Folder scanning, file moves and per-format preview widgets.

use adw::prelude::*;
use gtk::{gdk, gdk_pixbuf, gio, glib};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Image,
    Gif,
    Video,
    Audio,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Like,
    Dislike,
    Favorite,
}

impl Verdict {
    pub fn dir_name(self) -> &'static str {
        match self {
            Verdict::Like => "liked",
            Verdict::Dislike => "disliked",
            Verdict::Favorite => "favorites",
        }
    }
}

fn kind_of(path: &Path) -> Option<Kind> {
    let (ctype, _) = gio::content_type_guess(Some(path), None);
    let mime = gio::content_type_get_mime_type(&ctype)?;
    match mime.as_str() {
        "image/gif" => Some(Kind::Gif),
        // GTK can't draw vector or raw camera formats reliably; skip them.
        m if m.starts_with("image/") && !m.contains("svg") => Some(Kind::Image),
        m if m.starts_with("video/") => Some(Kind::Video),
        m if m.starts_with("audio/") => Some(Kind::Audio),
        _ => None,
    }
}

/// Top-level media files of `dir`, sorted by name. The verdict subfolders are
/// directories, so already sorted files are naturally left out.
pub fn scan(dir: &Path) -> std::io::Result<Vec<Item>> {
    let mut items: Vec<Item> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
        .filter_map(|path| kind_of(&path).map(|kind| Item { path, kind }))
        .collect();
    items.sort_by_cached_key(|i| i.path.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    Ok(items)
}

/// Moves `src` into `dir`, picking `name (N).ext` if the name is taken.
pub fn move_into(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = src.file_name().unwrap_or_default();
    let mut dest = dir.join(name);
    let stem = src.file_stem().unwrap_or_default().to_string_lossy();
    let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 1;
    while dest.exists() {
        dest = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::rename(src, &dest)?;
    Ok(dest)
}

/// Moves a file back to where it came from. Refuses to overwrite.
pub fn move_back(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    std::fs::rename(from, to)
}

pub struct Preview {
    pub widget: gtk::Widget,
    pub stream: Option<gtk::MediaStream>,
}

pub fn preview(item: &Item) -> Preview {
    match item.kind {
        Kind::Image => {
            let widget = match gdk::Texture::from_filename(&item.path) {
                Ok(tex) => {
                    let pic = picture();
                    pic.set_paintable(Some(&tex));
                    pic.upcast()
                }
                Err(e) => unsupported(&item.path, "Не удалось показать картинку", e.message()).upcast(),
            };
            Preview { widget, stream: None }
        }
        Kind::Gif => Preview { widget: gif(&item.path), stream: None },
        Kind::Video => {
            let pic = picture();
            let stream = crate::player::PlayStream::video(&item.path, &pic);
            stream.set_loop(true);
            let controls = gtk::MediaControls::new(Some(&stream));
            controls.add_css_class("osd");
            controls.add_css_class("video-controls");
            controls.set_valign(gtk::Align::End);
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&pic));
            overlay.add_overlay(&controls);
            overlay.add_overlay(&stream_error(stream.upcast_ref(), &item.path));
            Preview { widget: overlay.upcast(), stream: Some(stream.upcast()) }
        }
        Kind::Audio => audio(&item.path),
    }
}

/// What a card shows in place of a file it can't display: the reason and an
/// "Open in…" button that lets another app handle it.
fn unsupported(path: &Path, title: &str, reason: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 10);
    column.set_valign(gtk::Align::Center);
    column.set_halign(gtk::Align::Center);
    column.set_margin_start(24);
    column.set_margin_end(24);
    column.add_css_class("media-error");

    let icon = gtk::Image::from_icon_name("image-missing-symbolic");
    icon.set_pixel_size(64);
    column.append(&icon);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-4");
    column.append(&heading);
    let detail = gtk::Label::builder().label(reason).wrap(true).justify(gtk::Justification::Center).build();
    detail.add_css_class("dim-label");
    column.append(&detail);

    let open = gtk::Button::with_label("Открыть в…");
    open.add_css_class("pill");
    open.add_css_class("suggested-action");
    open.set_halign(gtk::Align::Center);
    open.set_margin_top(6);
    let file = gio::File::for_path(path);
    open.connect_clicked(move |b| open_with(b, &file));
    column.append(&open);
    column
}

/// Asks the desktop which app should open `file`.
fn open_with(button: &gtk::Button, file: &gio::File) {
    let launcher = gtk::FileLauncher::new(Some(file));
    launcher.set_always_ask(true);
    let window = button.root().and_downcast::<gtk::Window>();
    let toast_target = button.downgrade();
    launcher.launch(window.as_ref(), gio::Cancellable::NONE, move |res| {
        if let (Err(e), Some(b)) = (res, toast_target.upgrade()) {
            if !e.matches(gtk::DialogError::Dismissed) {
                let _ = b.activate_action("win.toast", Some(&e.message().to_variant()));
            }
        }
    });
}

/// Hidden until the stream fails, then shows why and offers another app.
fn stream_error(stream: &gtk::MediaStream, path: &Path) -> adw::Bin {
    let bin = adw::Bin::new();
    bin.set_visible(false);
    let show = {
        let path = path.to_path_buf();
        move |bin: &adw::Bin, stream: &gtk::MediaStream| {
            if let Some(e) = stream.error() {
                let reason = format!("{}\nВозможно, не хватает кодеков GStreamer (gst-libav, gst-plugins-bad).", e.message());
                bin.set_child(Some(&unsupported(&path, "Не удалось воспроизвести", &reason)));
                bin.set_visible(true);
            }
        }
    };
    show(&bin, stream);
    let weak = bin.downgrade();
    stream.connect_error_notify(move |s| {
        if let Some(bin) = weak.upgrade() {
            show(&bin, s);
        }
    });
    bin
}

fn picture() -> gtk::Picture {
    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Contain);
    pic.set_can_shrink(true);
    pic
}

fn gif(path: &Path) -> gtk::Widget {
    let pic = picture();
    match gdk_pixbuf::PixbufAnimation::from_file(path) {
        Ok(anim) if !anim.is_static_image() => {
            let iter = anim.iter(Some(SystemTime::now()));
            pic.set_paintable(Some(&texture(&iter.pixbuf())));
            animate(pic.downgrade(), iter);
        }
        _ => match gdk::Texture::from_filename(path) {
            Ok(tex) => pic.set_paintable(Some(&tex)),
            Err(e) => return unsupported(path, "Не удалось показать картинку", e.message()).upcast(),
        },
    }
    pic.upcast()
}

#[allow(deprecated)]
fn texture(pixbuf: &gdk_pixbuf::Pixbuf) -> gdk::Texture {
    gdk::Texture::for_pixbuf(pixbuf)
}

/// Steps the animation until the picture is gone.
fn animate(pic: glib::WeakRef<gtk::Picture>, iter: gdk_pixbuf::PixbufAnimationIter) {
    let delay = iter.delay_time().map_or(100, |d| d.as_millis() as u64).max(20);
    glib::timeout_add_local_once(Duration::from_millis(delay), move || {
        let Some(p) = pic.upgrade() else { return };
        if iter.advance(SystemTime::now()) {
            p.set_paintable(Some(&texture(&iter.pixbuf())));
        }
        animate(pic, iter);
    });
}

fn audio(path: &Path) -> Preview {
    use lofty::prelude::*;

    let stream = crate::player::PlayStream::audio(path);
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged.as_ref().and_then(|t| t.primary_tag().or_else(|| t.first_tag()));

    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_valign(gtk::Align::Center);
    column.set_margin_start(24);
    column.set_margin_end(24);

    let cover = tag
        .and_then(|t| t.pictures().first())
        .and_then(|p| gdk::Texture::from_bytes(&glib::Bytes::from(p.data())).ok());
    match cover {
        Some(tex) => {
            let pic = picture();
            pic.set_paintable(Some(&tex));
            pic.set_size_request(-1, 240);
            pic.add_css_class("cover");
            column.append(&pic);
        }
        None => {
            let icon = gtk::Image::from_icon_name("audio-x-generic-symbolic");
            icon.set_pixel_size(128);
            icon.add_css_class("dim-label");
            column.append(&icon);
        }
    }

    let fallback = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let title = tag.and_then(|t| t.title().map(|s| s.into_owned())).unwrap_or(fallback);
    let title = gtk::Label::builder().label(title).wrap(true).justify(gtk::Justification::Center).build();
    title.add_css_class("title-2");
    column.append(&title);
    if let Some(artist) = tag.and_then(|t| t.artist()) {
        let artist = gtk::Label::new(Some(&artist));
        artist.add_css_class("dim-label");
        column.append(&artist);
    }

    let controls = gtk::MediaControls::new(Some(&stream));
    column.append(&controls);
    column.append(&stream_error(stream.upcast_ref(), path));

    Preview { widget: column.upcast(), stream: Some(stream.upcast()) }
}
