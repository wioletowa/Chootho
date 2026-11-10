//! The main window: the card stack, gestures, keys, undo history and the
//! first-run tutorial.

use crate::card::SwipeCard;
use crate::media::{self, Item, Verdict};
use crate::tutorial::{self, STEPS};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const BACK_SCALE: f64 = 0.94;
/// How far the tutorial's hint tilts the card, in pixels.
const HINT_REACH: f64 = 70.0;

/// What a card shows: a file from the folder, or a tutorial cat.
enum Source {
    File(Item),
    Cat(usize),
}

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
        // Each stamp sits on the side the card is moving away from.
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

    fn load(&self, source: &Source) {
        self.clear();
        match source {
            Source::File(item) => {
                let preview = media::preview(item);
                self.bin.set_child(Some(&preview.widget));
                self.stream.replace(preview.stream);
                self.caption.set_label(&item.path.file_name().unwrap_or_default().to_string_lossy());
            }
            Source::Cat(step) => {
                let pic = gtk::Picture::for_paintable(&tutorial::step_texture(*step));
                pic.set_content_fit(gtk::ContentFit::Cover);
                pic.set_can_shrink(true);
                self.bin.set_child(Some(&pic));
                self.caption.set_label(&format!("cat{}.jpg", step + 1));
            }
        }
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
    /// Card offset when the current drag started (a hint may have tilted it).
    drag_base: (f64, f64),
    /// Tutorial mode: `idx` is the step and `items` stays empty.
    tutorial: bool,
    hint: Option<adw::Animation>,
    /// Bumped on every interaction so stale hint timers do nothing.
    hint_gen: u64,
    /// A folder given on the command line, opened once the tutorial ends.
    pending: Option<PathBuf>,
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    stack: gtk::Stack,
    title: adw::WindowTitle,
    toasts: adw::ToastOverlay,
    done: adw::StatusPage,
    undo_buttons: [gtk::Button; 2],
    welcome: gtk::Box,
    skip: gtk::Button,
    hint_label: gtk::Label,
    check: SwipeCard,
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
        let skip = gtk::Button::builder()
            .child(&adw::ButtonContent::builder().label("Пропустить").icon_name("go-next-symbolic").build())
            .tooltip_text("Пропустить обучение (Esc)")
            .visible(false)
            .build();
        skip.add_css_class("flat");
        header.pack_end(&skip);

        // Empty state.
        let empty_open = gtk::Button::with_label("Открыть папку");
        empty_open.add_css_class("pill");
        empty_open.add_css_class("suggested-action");
        empty_open.set_halign(gtk::Align::Center);
        let empty = adw::StatusPage::builder()
            .icon_name("folder-pictures-symbolic")
            .title("Выберите папку")
            .description("Или перетащите её в окно.")
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

        let hint_label = gtk::Label::builder().wrap(true).justify(gtk::Justification::Center).visible(false).build();
        hint_label.add_css_class("tutorial-hint");
        let cards = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cards.append(&hint_label);
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

        // Tutorial finale: a big check mark that pops in.
        let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
        check_icon.set_pixel_size(160);
        check_icon.add_css_class("tutorial-check");
        let check = SwipeCard::new(&check_icon);
        let ready = gtk::Label::new(Some("Готово! Теперь выбери папку"));
        ready.add_css_class("title-2");
        let finale = gtk::Box::new(gtk::Orientation::Vertical, 18);
        finale.set_valign(gtk::Align::Center);
        finale.append(&check);
        finale.append(&ready);
        stack.add_named(&finale, Some("tutorial-done"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let (welcome, start_btn) = tutorial::welcome();
        welcome.set_visible(false);
        let layers = gtk::Overlay::new();
        layers.set_child(Some(&toasts));
        layers.add_overlay(&welcome);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&layers));
        window.set_content(Some(&view));

        let this = Rc::new(Window {
            window,
            stack,
            title,
            toasts,
            done,
            undo_buttons: [undo_btn.clone(), header_undo.clone()],
            welcome,
            skip: skip.clone(),
            hint_label,
            check,
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
        let w = this.clone();
        start_btn.connect_clicked(move |_| w.begin_tutorial());
        let w = this.clone();
        skip.connect_clicked(move |_| w.finish_tutorial());
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
                return;
            }
            // A tutorial hint may be mid-tilt: catch the card where it is.
            w.stop_hint();
            w.state.borrow_mut().drag_base = w.front.card.offset();
        });
        let w = self.clone();
        drag.connect_drag_update(move |g, dx, dy| {
            if dx.hypot(dy) > 8.0 {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
            let (bx, by) = w.state.borrow().drag_base;
            w.set_offset(bx + dx, by + dy);
        });
        let w = self.clone();
        drag.connect_drag_end(move |_, _, _| {
            let (dx, dy) = w.front.card.offset();
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
            if w.welcome.is_visible() {
                match key {
                    gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => w.begin_tutorial(),
                    gdk::Key::Escape => w.finish_tutorial(),
                    _ => return glib::Propagation::Proceed,
                }
                return glib::Propagation::Stop;
            }
            let page = w.stack.visible_child_name();
            let sorting = page.as_deref() == Some("cards");
            match key {
                gdk::Key::Escape if w.state.borrow().tutorial => w.finish_tutorial(),
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
        self.leave_tutorial();
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

    fn current(&self, offset: usize) -> Option<Source> {
        let s = self.state.borrow();
        let i = s.idx + offset;
        if s.tutorial {
            (i < STEPS.len()).then_some(Source::Cat(i))
        } else {
            s.items.get(i).cloned().map(Source::File)
        }
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
        self.skip.set_visible(s.tutorial);
        self.hint_label.set_visible(s.tutorial && s.idx < STEPS.len());
        for b in &self.undo_buttons {
            b.set_visible(!s.tutorial);
        }
        if s.tutorial {
            self.title.set_title("Обучение");
            if let Some(step) = STEPS.get(s.idx) {
                self.title.set_subtitle(&format!("{} из {}", s.idx + 1, STEPS.len()));
                self.hint_label.set_label(step.hint);
                self.stack.set_visible_child_name("cards");
            } else {
                self.title.set_subtitle("");
                self.stack.set_visible_child_name("tutorial-done");
            }
            return;
        }
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
        self.schedule_hint(5000);
    }

    /// Flies the front card off in the verdict's direction, then moves the file.
    fn decide(self: &Rc<Self>, verdict: Verdict) {
        self.stop_hint();
        self.finish_anim();
        if !self.front.has_content() {
            return;
        }
        let expected = {
            let s = self.state.borrow();
            s.tutorial.then(|| STEPS.get(s.idx).map(|step| step.verdict)).flatten()
        };
        if expected.is_some_and(|e| e != verdict) {
            // Wrong way during the tutorial: bounce back and show it again.
            self.spring_home();
            self.schedule_hint(900);
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
        if self.state.borrow().tutorial {
            self.state.borrow_mut().idx += 1;
            self.promote();
            if self.state.borrow().idx < STEPS.len() {
                self.schedule_hint(700);
            } else {
                self.celebrate();
            }
            return;
        }
        let (item, folder) = {
            let s = self.state.borrow();
            match (self.current(0), s.folder.clone()) {
                (Some(Source::File(i)), Some(f)) => (i, f),
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
        if self.state.borrow().tutorial {
            return;
        }
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

    /// Shows the welcome panel; `pending` is opened when the tutorial ends.
    pub fn start_tutorial(&self, pending: Option<PathBuf>) {
        {
            let mut s = self.state.borrow_mut();
            s.pending = pending;
            s.tutorial = true;
            s.idx = 0;
        }
        // The first cat already waits under the translucent panel.
        self.load_front();
        self.load_back();
        self.refresh();
        self.welcome.set_visible(true);
    }

    fn begin_tutorial(self: &Rc<Self>) {
        self.welcome.set_visible(false);
        self.schedule_hint(700);
    }

    /// Big check mark, then on to normal use.
    fn celebrate(self: &Rc<Self>) {
        self.check.set_scale(0.3);
        let card = self.check.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| card.set_scale(v));
        let pop = adw::SpringAnimation::new(&self.check, 0.3, 1.0, adw::SpringParams::new(0.5, 1.0, 300.0), target);
        pop.play();
        let w = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(1600), move || {
            if w.state.borrow().tutorial {
                w.finish_tutorial();
            }
        });
    }

    /// Ends or skips the tutorial for good and opens the pending folder.
    fn finish_tutorial(self: &Rc<Self>) {
        let pending = self.state.borrow_mut().pending.take();
        match pending {
            Some(dir) => self.open(&dir),
            None => {
                self.leave_tutorial();
                self.refresh();
            }
        }
    }

    fn leave_tutorial(&self) {
        let was_on = self.welcome.is_visible() || self.state.borrow().tutorial;
        if !was_on {
            return;
        }
        tutorial::mark_done();
        self.welcome.set_visible(false);
        self.stop_hint();
        let tutorial = std::mem::replace(&mut self.state.borrow_mut().tutorial, false);
        if tutorial {
            self.finish_anim();
            self.front.clear();
            self.back.clear();
            self.back.card.set_visible(false);
            self.state.borrow_mut().idx = 0;
        }
    }

    /// Plays the tutorial hint after `ms` unless something happens first.
    fn schedule_hint(self: &Rc<Self>, ms: u64) {
        let generation = {
            let mut s = self.state.borrow_mut();
            if !s.tutorial {
                return;
            }
            s.hint_gen += 1;
            s.hint_gen
        };
        let w = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if w.state.borrow().hint_gen == generation {
                w.play_hint();
            }
        });
    }

    /// Tilts the card toward the step's direction and back.
    fn play_hint(self: &Rc<Self>) {
        let verdict = {
            let s = self.state.borrow();
            match STEPS.get(s.idx) {
                Some(step) if s.tutorial => step.verdict,
                _ => return,
            }
        };
        if self.busy() || self.front.card.offset() != (0.0, 0.0) {
            return self.schedule_hint(1000);
        }
        let (ux, uy) = match verdict {
            Verdict::Like => (1.0, 0.0),
            Verdict::Dislike => (-1.0, 0.0),
            Verdict::Favorite => (0.0, -1.0),
        };
        let w = self.clone();
        let target = adw::CallbackAnimationTarget::new(move |t| {
            let reach = HINT_REACH * (t * std::f64::consts::PI).sin();
            w.set_offset(ux * reach, uy * reach);
        });
        let hint = adw::TimedAnimation::new(&self.front.card, 0.0, 1.0, 1300, target);
        hint.set_easing(adw::Easing::Linear);
        let w = self.clone();
        hint.connect_done(move |_| w.schedule_hint(5000));
        self.state.borrow_mut().hint = Some(hint.clone().upcast());
        hint.play();
    }

    /// Freezes a running hint where it is, so a drag can take over from there.
    fn stop_hint(&self) {
        let hint = {
            let mut s = self.state.borrow_mut();
            s.hint_gen += 1;
            s.hint.take()
        };
        if let Some(h) = hint {
            h.pause();
        }
    }
}
