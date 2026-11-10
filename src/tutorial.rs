//! First-run tutorial: embedded assets, the welcome panel and the "seen" mark.

use crate::media::Verdict;
use adw::prelude::*;
use gtk::{gdk, glib};
use std::path::PathBuf;

pub struct Step {
    image: &'static [u8],
    pub verdict: Verdict,
    pub hint: &'static str,
}

pub const STEPS: [Step; 3] = [
    Step {
        image: include_bytes!("../assets/tutorial/cat1.jpg"),
        verdict: Verdict::Like,
        hint: "Like it? Swipe right or press →",
    },
    Step {
        image: include_bytes!("../assets/tutorial/cat2.jpg"),
        verdict: Verdict::Dislike,
        hint: "Not for you? Swipe left or press ←",
    },
    Step {
        image: include_bytes!("../assets/tutorial/cat3.jpg"),
        verdict: Verdict::Favorite,
        hint: "A keeper? Swipe up or press Space to favorite",
    },
];

const ICON: &[u8] = include_bytes!("../assets/icons/chootho.png");

fn texture(bytes: &'static [u8]) -> gdk::Texture {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(bytes)).expect("embedded image decodes")
}

pub fn step_texture(step: usize) -> gdk::Texture {
    texture(STEPS[step].image)
}

/// An empty marker file; the only thing Chootho ever writes outside the
/// folders it sorts. `CHOOTHO_TUTORIAL=1` shows the tutorial regardless.
fn mark_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local/state"));
    state.join("chootho-tutorial-done")
}

pub fn should_show() -> bool {
    std::env::var_os("CHOOTHO_TUTORIAL").is_some() || !mark_path().exists()
}

pub fn mark_done() {
    let path = mark_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::File::create(path);
}

/// The translucent first-run panel: name, icon, the three directions and
/// a "Start" button, returned so the window can connect it.
pub fn welcome() -> (gtk::Box, gtk::Button) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 18);
    column.set_valign(gtk::Align::Center);
    column.set_halign(gtk::Align::Center);

    let title = gtk::Label::new(Some("Chootho"));
    title.add_css_class("welcome-title");
    column.append(&title);

    let icon = gtk::Image::from_paintable(Some(&texture(ICON)));
    icon.set_pixel_size(128);
    column.append(&icon);

    let arrow = |text: &str, class: &str| {
        let label = gtk::Label::new(Some(text));
        label.add_css_class("welcome-arrow");
        label.add_css_class(class);
        label
    };
    let diagram = gtk::Grid::builder()
        .row_spacing(6)
        .column_spacing(18)
        .halign(gtk::Align::Center)
        .margin_top(18)
        .build();
    let up = gtk::Box::new(gtk::Orientation::Vertical, 0);
    up.append(&arrow("favorite", "fav"));
    up.append(&arrow("↑", "fav"));
    diagram.attach(&up, 1, 0, 1, 1);
    diagram.attach(&arrow("← nope", "nope"), 0, 1, 1, 1);
    // Empty middle keeps the arrows apart, pointing at the card beneath.
    let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
    gap.set_size_request(72, 48);
    diagram.attach(&gap, 1, 1, 1, 1);
    diagram.attach(&arrow("like →", "like"), 2, 1, 1, 1);
    column.append(&diagram);

    let keys = gtk::Label::new(Some("↓ or Ctrl+Z to undo"));
    keys.add_css_class("dim-label");
    column.append(&keys);

    let start = gtk::Button::with_label("Start");
    start.add_css_class("pill");
    start.add_css_class("suggested-action");
    start.set_halign(gtk::Align::Center);
    start.set_margin_top(12);
    column.append(&start);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("welcome");
    panel.append(&column);
    column.set_vexpand(true);
    (panel, start)
}
