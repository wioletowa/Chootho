mod card;
mod gpu;
mod media;
mod player;
mod tutorial;
mod window;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "io.github.whyoolw.Chootho";

const CSS: &str = r#"
.card-frame {
    background-color: #141414;
    border-radius: 24px;
    box-shadow: 0 6px 22px alpha(black, 0.28);
}
.back .card-frame { opacity: 0.55; }
.card-frame .caption {
    color: white;
    background-color: alpha(black, 0.45);
    border-radius: 999px;
    padding: 4px 12px;
    margin: 14px;
    font-weight: 600;
}
.card-frame .title-2, .card-frame .dim-label { color: white; }
.cover { border-radius: 12px; }
.video-controls {
    margin: 12px;
    border-radius: 999px;
    padding: 0 6px;
}
.media-error { color: white; }
.stamp {
    font-size: 30px;
    font-weight: 900;
    border: 4px solid;
    border-radius: 12px;
    padding: 2px 14px;
    margin: 56px 26px;
    background-color: alpha(black, 0.25);
}
.stamp.like { color: #57e389; border-color: #57e389; transform: rotate(-14deg); }
.stamp.nope { color: #ff6b6b; border-color: #ff6b6b; transform: rotate(14deg); }
.welcome { background-color: alpha(@window_bg_color, 0.84); }
.welcome-title { font-size: 44px; font-weight: 900; }
.welcome-arrow { font-size: 22px; font-weight: 800; }
.welcome-arrow.like { color: #57e389; }
.welcome-arrow.nope { color: #ff6b6b; }
.welcome-arrow.fav { color: #f8e45c; }
.tutorial-hint { font-size: 18px; font-weight: 700; margin: 12px 24px 0; }
.tutorial-check { color: #57e389; }
.stamp.fav  { color: #f8e45c; border-color: #f8e45c; margin-bottom: 40px; }
.action {
    min-width: 64px;
    min-height: 64px;
    font-size: 28px;
    font-weight: 700;
}
.action.undo { min-width: 48px; min-height: 48px; font-size: 20px; }
.action.fav { min-width: 54px; min-height: 54px; font-size: 24px; }
.action.like { color: #2ec27e; }
.action.nope { color: #e01b24; }
.action.fav { color: #e5a50a; }
"#;

fn main() -> glib::ExitCode {
    gpu::prefer_integrated();
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();

    app.connect_startup(|_| {
        if let Err(e) = gst::init() {
            eprintln!("GStreamer init failed: {e}");
        }
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("no display"),
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
    app.connect_activate(|app| present(app, None));
    app.connect_open(|app, files, _| present(app, files.first().and_then(|f| f.path())));
    app.run()
}

fn present(app: &adw::Application, dir: Option<std::path::PathBuf>) {
    let win = window::Window::new(app);
    if tutorial::should_show() {
        win.start_tutorial(dir);
    } else if let Some(dir) = dir {
        win.open(&dir);
    }
    win.window.present();
}
