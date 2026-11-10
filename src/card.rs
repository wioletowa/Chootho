//! A single-child widget that renders its child shifted, rotated and scaled,
//! so the card can follow the pointer without relayouting anything else.

use gtk::glib;
use gtk::graphene::Point;
use gtk::gsk::Transform;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    pub struct SwipeCard {
        pub child: RefCell<Option<gtk::Widget>>,
        pub dx: Cell<f64>,
        pub dy: Cell<f64>,
        pub scale: Cell<f64>,
    }

    impl Default for SwipeCard {
        fn default() -> Self {
            Self {
                child: RefCell::default(),
                dx: Cell::new(0.0),
                dy: Cell::new(0.0),
                scale: Cell::new(1.0),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SwipeCard {
        const NAME: &'static str = "ChoothoSwipeCard";
        type Type = super::SwipeCard;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SwipeCard {
        fn dispose(&self) {
            if let Some(child) = self.child.take() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for SwipeCard {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.child.borrow().as_ref() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let Some(child) = self.child.borrow().clone() else { return };
            let (w, h) = (width as f32, height as f32);
            let (dx, dy) = (self.dx.get() as f32, self.dy.get() as f32);
            let scale = self.scale.get() as f32;
            // Tilt with horizontal travel, like a card held at the bottom.
            let angle = if w > 0.0 { dx / w * 14.0 } else { 0.0 };
            let t = Transform::new()
                .translate(&Point::new(w / 2.0 + dx, h / 2.0 + dy))
                .rotate(angle)
                .scale(scale, scale)
                .translate(&Point::new(-w / 2.0, -h / 2.0));
            child.allocate(width, height, baseline, Some(t));
        }
    }
}

glib::wrapper! {
    pub struct SwipeCard(ObjectSubclass<imp::SwipeCard>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SwipeCard {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let card: Self = glib::Object::new();
        child.set_parent(&card);
        card.imp().child.replace(Some(child.clone().upcast()));
        card
    }

    pub fn offset(&self) -> (f64, f64) {
        (self.imp().dx.get(), self.imp().dy.get())
    }

    pub fn set_offset(&self, dx: f64, dy: f64) {
        self.imp().dx.set(dx);
        self.imp().dy.set(dy);
        self.queue_allocate();
    }

    pub fn set_scale(&self, scale: f64) {
        self.imp().scale.set(scale);
        self.queue_allocate();
    }
}
