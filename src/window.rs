//! The main window: the card stack, gestures, keys and undo history.

use crate::card::SwipeCard;
use crate::media::{self, Item, Verdict};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const BACK_SCALE: f64 = 0.94;

/// One card position (front or back) with its frame and verdict stamps.
struct Slot {
    card: SwipeCard,
    bin: adw::Bin,
    caption: gtk::Label,
    like: gtk::Label,
    nope: gtk::Label,
    fav: gtk::Label,
    stream: RefCell<Option<gtk::MediaStream>>,
}

impl Slot {
    fn new() -> Self {
        let bin = adw::Bin::new();
        let frame = gtk::Overlay::new();
        frame.add_css_class("card-frame");
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.set_child(Some(&bin));

        let stamp = |text: &str, class: &str, halign, valign| {
            let label = gtk::Label::new(Some(text));
            label.add_css_class("stamp");
            label.add_css_class(class);
            label.set_halign(halign);
            label.set_valign(valign);
            label.set_opacity(0.0);
            label.set_can_target(false);
            frame.add_overlay(&label);
            label
        };
        let like = stamp("ЛАЙК", "like", gtk::Align::Start, gtk::Align::Start);
        let nope = stamp("НЕТ", "nope", gtk::Align::End, gtk::Align::Start);
        let fav = stamp("★ ИЗБРАННОЕ", "fav", gtk::Align::Center, gtk::Align::End);

        let caption = gtk::Label::new(None);
        caption.add_css_class("caption");
        caption.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        caption.set_halign(gtk::Align::Start);
        caption.set_valign(gtk::Align::Start);
        caption.set_can_target(false);
        frame.add_overlay(&caption);

        let card = SwipeCard::new(&frame);
        card.set_hexpand(true);
        card.set_vexpand(true);
        Slot { card, bin, caption, like, nope, fav, stream: RefCell::default() }
    }

    fn clear(&self) {
        self.pause();
        self.stream.take();
        self.bin.set_child(None::<&gtk::Widget>);
        self.caption.set_label("");
    }

    fn load(&self, item: &Item) {
        self.clear();
        let preview = media::preview(item);
        self.bin.set_child(Some(&preview.widget));
        self.stream.replace(preview.stream);
        self.caption.set_label(&item.path.file_name().unwrap_or_default().to_string_lossy());
    }

    /// Moves this slot's content into `other`, leaving this one empty.
    fn hand_to(&self, other: &Slot) {
        other.clear();
        let child = self.bin.child();
        self.bin.set_child(None::<&gtk::Widget>);
        other.bin.set_child(child.as_ref());
        other.stream.replace(self.stream.take());
        other.caption.set_label(&self.caption.label());
        self.caption.set_label("");
    }

    fn play(&self) {
        if let Some(s) = self.stream.borrow().as_ref() {
            s.play();
        }
    }

    fn pause(&self) {
        if let Some(s) = self.stream.borrow().as_ref() {
            s.pause();
        }
    }

    fn has_content(&self) -> bool {
        self.bin.child().is_some()
    }
}

struct Moved {
    from: PathBuf,
    to: PathBuf,
    verdict: Verdict,
}

#[derive(Default)]
struct State {
    folder: Option<PathBuf>,
    items: Vec<Item>,
    idx: usize,
    history: Vec<Moved>,
    anim: Option<adw::Animation>,
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    done: adw::StatusPage,
    undo_buttons: [gtk::Button; 2],
    front: Slot,
    back: Slot,
    state: RefCell<State>,
}

impl Window {
    pub fn new(app: &adw::Application) -> Rc<Self> {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Chootho")
            .default_width(560)
            .default_height(760)
            .build();

        let title = adw::WindowTitle::new("Chootho", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        let open_btn = gtk::Button::from_icon_name("folder-open-symbolic");
        open_btn.set_tooltip_text(Some("Открыть папку (Ctrl+O)"));
        header.pack_start(&open_btn);
        let header_undo = gtk::Button::from_icon_name("edit-undo-symbolic");
        header_undo.set_tooltip_text(Some("Отменить (Ctrl+Z, ↓)"));
        header.pack_end(&header_undo);

        // Empty state.
        let empty_open = gtk::Button::with_label("Открыть папку");
        empty_open.add_css_class("pill");
        empty_open.add_css_class("suggested-action");
        empty_open.set_halign(gtk::Align::Center);
        let empty = adw::StatusPage::builder()
            .icon_name("folder-pictures-symbolic")
            .title("Выберите папку")
            .description("Или перетащите её в окно.\n← нет  ·  → лайк  ·  пробел/↑ избранное  ·  ↓ отмена")
            .child(&empty_open)
            .build();

        // Card deck: both cards share one grid cell, front drawn last.
        let front = Slot::new();
        let back = Slot::new();
        back.card.set_scale(BACK_SCALE);
        back.card.set_can_target(false);
        back.card.add_css_class("back");
        let deck = gtk::Grid::new();
        deck.attach(&back.card, 0, 0, 1, 1);
        deck.attach(&front.card, 0, 0, 1, 1);
        deck.set_margin_top(12);
        deck.set_margin_start(18);
        deck.set_margin_end(18);

        let action = |label: &str, class: &str, tip: &str| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("circular");
            b.add_css_class("action");
            b.add_css_class(class);
            b.set_tooltip_text(Some(tip));
            b.set_valign(gtk::Align::Center);
            b
        };
        let undo_btn = action("↶", "undo", "Отменить (Ctrl+Z, ↓)");
        let nope_btn = action("✕", "nope", "Нет (←)");
        let fav_btn = action("★", "fav", "В избранное (пробел, ↑)");
        let like_btn = action("♥", "like", "Лайк (→)");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        buttons.set_halign(gtk::Align::Center);
        buttons.set_margin_top(14);
        buttons.set_margin_bottom(18);
        for b in [&undo_btn, &nope_btn, &fav_btn, &like_btn] {
            buttons.append(b);
        }

        let cards = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cards.append(&deck);
        cards.append(&buttons);

        let done_open = gtk::Button::with_label("Открыть другую папку");
        done_open.add_css_class("pill");
        done_open.set_halign(gtk::Align::Center);
        let done = adw::StatusPage::builder()
            .icon_name("object-select-symbolic")
            .title("Всё рассортировано")
            .child(&done_open)
            .build();

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&cards, Some("cards"));
        stack.add_named(&done, Some("done"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&toasts));
        window.set_content(Some(&view));

        let this = Rc::new(Window {
            window,
            stack,
            title,
            toasts,
            done,
            undo_buttons: [undo_btn.clone(), header_undo.clone()],
            front,
            back,
            state: RefCell::default(),
        });

        for b in [&open_btn, &empty_open, &done_open] {
            let w = this.clone();
            b.connect_clicked(move |_| w.choose_folder());
        }
        for b in [&undo_btn, &header_undo] {
            let w = this.clone();
            b.connect_clicked(move |_| w.undo());
        }
        for (b, v) in [(&nope_btn, Verdict::Dislike), (&fav_btn, Verdict::Favorite), (&like_btn, Verdict::Like)] {
            let w = this.clone();
            b.connect_clicked(move |_| w.decide(v));
        }

        // Lets card contents (e.g. a failed "Open in…") report errors.
        let toast = gio::SimpleAction::new("toast", Some(glib::VariantTy::STRING));
        let w = this.clone();
        toast.connect_activate(move |_, text| {
            if let Some(text) = text.and_then(|t| t.str()) {
                w.toast(text);
            }
        });
        this.window.add_action(&toast);

        this.setup_drag();
        this.setup_keys();
        this.setup_drop();
        this.refresh();
        this
    }

    fn setup_drag(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        // Capture, so swipes work over video controls too; we only claim the
        // sequence once it has clearly become a drag, so clicks still pass.
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = self.clone();
        drag.connect_drag_begin(move |g, _, _| {
            if w.busy() || !w.front.has_content() {
                g.set_state(gtk::EventSequenceState::Denied);
            }
        });
        let w = self.clone();
        drag.connect_drag_update(move |g, dx, dy| {
            if dx.hypot(dy) > 8.0 {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
            w.set_offset(dx, dy);
        });
        let w = self.clone();
        drag.connect_drag_end(move |_, dx, dy| {
            let threshold = (w.front.card.width() as f64 * 0.25).max(90.0);
            if -dy > threshold && -dy > dx.abs() {
                w.decide(Verdict::Favorite);
            } else if dx > threshold {
                w.decide(Verdict::Like);
            } else if dx < -threshold {
                w.decide(Verdict::Dislike);
            } else {
                w.spring_home();
            }
        });
        self.front.card.add_controller(drag);
    }

    fn setup_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
            let page = w.stack.visible_child_name();
            let sorting = page.as_deref() == Some("cards");
            match key {
                gdk::Key::o | gdk::Key::O if ctrl => w.choose_folder(),
                gdk::Key::z | gdk::Key::Z if ctrl => w.undo(),
                _ if ctrl => return glib::Propagation::Proceed,
                gdk::Key::Down | gdk::Key::BackSpace => w.undo(),
                gdk::Key::Left if sorting => w.decide(Verdict::Dislike),
                gdk::Key::Right if sorting => w.decide(Verdict::Like),
                gdk::Key::Up | gdk::Key::space if sorting => w.decide(Verdict::Favorite),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }

    fn setup_drop(self: &Rc<Self>) {
        let drop = gtk::DropTarget::new(gio::File::static_type(), gdk::DragAction::COPY);
        let w = self.clone();
        drop.connect_drop(move |_, value, _, _| {
            let Some(path) = value.get::<gio::File>().ok().and_then(|f| f.path()) else { return false };
            let dir = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or(path) };
            w.open(&dir);
            true
        });
        self.window.add_controller(drop);
    }

    fn choose_folder(self: &Rc<Self>) {
        let dialog = gtk::FileDialog::builder().title("Папка с медиа").modal(true).build();
        if let Some(dir) = self.state.borrow().folder.as_ref() {
            dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
        }
        let w = self.clone();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Some(path) = res.ok().and_then(|f| f.path()) {
                w.open(&path);
            }
        });
    }

    pub fn open(self: &Rc<Self>, dir: &Path) {
        self.finish_anim();
        let items = match media::scan(dir) {
            Ok(items) => items,
            Err(e) => return self.toast(&format!("Не удалось открыть папку: {e}")),
        };
        self.front.clear();
        self.back.clear();
        {
            let mut s = self.state.borrow_mut();
            *s = State { folder: Some(dir.to_path_buf()), items, ..State::default() };
        }
        if self.state.borrow().items.is_empty() {
            self.toast("В этой папке нет картинок, видео или аудио");
        }
        self.load_front();
        self.load_back();
        self.refresh();
    }

    fn current(&self, offset: usize) -> Option<Item> {
        let s = self.state.borrow();
        s.items.get(s.idx + offset).cloned()
    }

    fn load_front(&self) {
        self.front.card.set_offset(0.0, 0.0);
        self.front.card.set_scale(1.0);
        match self.current(0) {
            Some(item) => {
                self.front.load(&item);
                self.front.play();
            }
            None => self.front.clear(),
        }
        self.update_stamps();
    }

    fn load_back(&self) {
        match self.current(1) {
            Some(item) => self.back.load(&item),
            None => self.back.clear(),
        }
        self.back.card.set_visible(self.back.has_content());
    }

    /// Updates the title, counters, undo sensitivity and the visible page.
    fn refresh(&self) {
        let s = self.state.borrow();
        let name = s.folder.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
        self.title.set_title(name.as_deref().unwrap_or("Chootho"));
        let page = match (&s.folder, s.idx < s.items.len()) {
            (None, _) => "empty",
            (Some(_), true) => "cards",
            (Some(_), false) if s.items.is_empty() && s.history.is_empty() => "empty",
            (Some(_), false) => "done",
        };
        self.title.set_subtitle(&match page {
            "cards" => format!("{} из {}", s.idx + 1, s.items.len()),
            _ => String::new(),
        });
        for b in &self.undo_buttons {
            b.set_sensitive(!s.history.is_empty());
        }
        if page == "done" {
            let count = |v| s.history.iter().filter(|m| m.verdict == v).count();
            self.done.set_description(Some(&format!(
                "♥ {}   ★ {}   ✕ {}\nФайлы разложены по папкам liked, favorites и disliked.",
                count(Verdict::Like),
                count(Verdict::Favorite),
                count(Verdict::Dislike),
            )));
        }
        self.stack.set_visible_child_name(page);
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn busy(&self) -> bool {
        self.state.borrow().anim.as_ref().is_some_and(|a| a.state() == adw::AnimationState::Playing)
    }

    /// Jumps any running animation to its end so a new action starts clean.
    fn finish_anim(&self) {
        let anim = self.state.borrow_mut().anim.take();
        if let Some(a) = anim {
            a.skip();
        }
    }

    fn set_anim(&self, anim: impl IsA<adw::Animation>) {
        let anim = anim.upcast();
        self.state.borrow_mut().anim = Some(anim.clone());
        anim.play();
    }

    fn set_offset(&self, dx: f64, dy: f64) {
        self.front.card.set_offset(dx, dy);
        self.update_stamps();
    }

    fn update_stamps(&self) {
        let (dx, dy) = self.front.card.offset();
        let f = |v: f64| (v / 110.0).clamp(0.0, 1.0);
        let up = -dy > dx.abs();
        self.front.like.set_opacity(if up { 0.0 } else { f(dx) });
        self.front.nope.set_opacity(if up { 0.0 } else { f(-dx) });
        self.front.fav.set_opacity(if up { f(-dy) } else { 0.0 });
    }

    fn spring_home(self: &Rc<Self>) {
        let (sx, sy) = self.front.card.offset();
        let w = self.clone();
        let target = adw::CallbackAnimationTarget::new(move |t| w.set_offset(sx * t, sy * t));
        let anim = adw::SpringAnimation::new(
            &self.front.card,
            1.0,
            0.0,
            adw::SpringParams::new(0.62, 1.0, 420.0),
            target,
        );
        self.set_anim(anim);
    }

    /// Flies the front card off in the verdict's direction, then moves the file.
    fn decide(self: &Rc<Self>, verdict: Verdict) {
        self.finish_anim();
        if !self.front.has_content() {
            return;
        }
        let (sx, sy) = self.front.card.offset();
        let (ex, ey) = self.offscreen(verdict, sy);
        let w = self.clone();
        let target = adw::CallbackAnimationTarget::new(move |t| {
            w.set_offset(sx + (ex - sx) * t, sy + (ey - sy) * t)
        });
        let anim = adw::TimedAnimation::new(&self.front.card, 0.0, 1.0, 220, target);
        anim.set_easing(adw::Easing::EaseOutCubic);
        let w = self.clone();
        anim.connect_done(move |_| w.commit(verdict));
        self.set_anim(anim);
    }

    fn offscreen(&self, verdict: Verdict, dy: f64) -> (f64, f64) {
        let width = self.window.width().max(self.front.card.width()) as f64;
        let height = self.window.height().max(self.front.card.height()) as f64;
        match verdict {
            Verdict::Like => (width * 1.2, dy + 40.0),
            Verdict::Dislike => (-width * 1.2, dy + 40.0),
            Verdict::Favorite => (0.0, -height * 1.2),
        }
    }

    fn commit(self: &Rc<Self>, verdict: Verdict) {
        let (item, folder) = {
            let s = self.state.borrow();
            match (self.current(0), s.folder.clone()) {
                (Some(i), Some(f)) => (i, f),
                _ => return,
            }
        };
        match media::move_into(&item.path, &folder.join(verdict.dir_name())) {
            Ok(to) => {
                let mut s = self.state.borrow_mut();
                s.history.push(Moved { from: item.path, to, verdict });
                s.idx += 1;
            }
            Err(e) => {
                self.toast(&format!("Не удалось переместить файл: {e}"));
                self.spring_home();
                return;
            }
        }
        self.promote();
    }

    /// Brings the back card forward and starts preparing the next one.
    fn promote(self: &Rc<Self>) {
        self.front.clear();
        self.front.card.set_offset(0.0, 0.0);
        self.update_stamps();
        if self.back.has_content() {
            self.back.hand_to(&self.front);
            self.front.play();
            self.front.card.set_scale(BACK_SCALE);
            let card = self.front.card.clone();
            let target = adw::CallbackAnimationTarget::new(move |s| card.set_scale(s));
            let anim = adw::TimedAnimation::new(&self.front.card, BACK_SCALE, 1.0, 160, target);
            anim.set_easing(adw::Easing::EaseOutCubic);
            self.set_anim(anim);
        } else {
            self.load_front();
        }
        self.back.card.set_visible(false);
        self.refresh();
        // Decode the next file after this frame so the swap itself stays smooth.
        let w = self.clone();
        glib::idle_add_local_once(move || w.load_back());
    }

    fn undo(self: &Rc<Self>) {
        self.finish_anim();
        let Some(moved) = self.state.borrow_mut().history.pop() else { return };
        if let Err(e) = media::move_back(&moved.to, &moved.from) {
            self.toast(&format!("Не удалось вернуть файл: {e}"));
            self.state.borrow_mut().history.push(moved);
            return;
        }
        self.state.borrow_mut().idx -= 1;

        // The current card goes back under the returning one.
        if self.front.has_content() {
            self.front.pause();
            self.front.hand_to(&self.back);
        } else {
            self.back.clear();
        }
        self.back.card.set_visible(self.back.has_content());
        self.load_front();
        self.refresh();

        let (sx, sy) = self.offscreen(moved.verdict, 0.0);
        self.set_offset(sx, sy);
        self.spring_home();
    }
}
