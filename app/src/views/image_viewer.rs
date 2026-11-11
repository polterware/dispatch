//! The image viewer: a run's picture opened whole, from its thumbnail in the transcript. It
//! is polterware-ui's `ImageViewer` modal, with Dispatch's effects for its two actions:
//! Reveal in Finder opens the copy's folder with the file selected, and Copy puts the image
//! itself on the clipboard, falling back to its path when the bytes cannot be read. Close,
//! Escape, or a press on the scrim closes it.

use std::path::Path;
use std::process::Command;

use gpui::{
    ClipboardItem, Context, DispatchPhase, Entity, Image, ImageFormat, IntoElement, KeyDownEvent,
    Render, Window, canvas, div, prelude::*,
};

use crate::models::transcript::TranscriptImage;
use crate::motion::{Animator, SpringTier, SpringValue};
use crate::state::AppState;
use crate::ui::{Icon, ImageAction, ImageData, ImageViewer as Viewer, ToastTone};

pub struct ImageViewerView {
    state: Entity<AppState>,
    image: Option<TranscriptImage>,
    open: bool,
    opacity: SpringValue,
    animator: Animator,
}

impl ImageViewerView {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            image: None,
            open: false,
            opacity: SpringValue::new(0.0, SpringTier::Fast),
            animator: Animator::new(),
        }
    }

    /// Opens the viewer on `image`, replacing whatever it showed.
    pub fn open(&mut self, image: TranscriptImage, cx: &mut Context<Self>) {
        self.image = Some(image);
        self.open = true;
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open || self.opacity.value() > 0.01
    }

    /// Opens the copy's folder in Finder with the file selected.
    fn reveal(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.image.as_ref().map(|image| image.path.clone()) else {
            return;
        };
        let opened = Command::new("/usr/bin/open")
            .args(["-R", &path])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !opened {
            self.toast("Could not reveal the image", cx);
        }
    }

    /// Puts the image on the clipboard; a picture no format covers falls back to its path.
    fn copy(&mut self, cx: &mut Context<Self>) {
        let Some(image) = self.image.clone() else {
            return;
        };
        let format = match image.path.rsplit('.').next() {
            Some(extension) if extension.eq_ignore_ascii_case("png") => Some(ImageFormat::Png),
            Some(extension) if extension.eq_ignore_ascii_case("jpg") => Some(ImageFormat::Jpeg),
            Some(extension) if extension.eq_ignore_ascii_case("jpeg") => Some(ImageFormat::Jpeg),
            Some(extension) if extension.eq_ignore_ascii_case("webp") => Some(ImageFormat::Webp),
            Some(extension) if extension.eq_ignore_ascii_case("gif") => Some(ImageFormat::Gif),
            _ => None,
        };
        let copied = format
            .and_then(|format| {
                std::fs::read(Path::new(&image.path))
                    .ok()
                    .map(|bytes| Image::from_bytes(format, bytes))
            })
            .map(|picture| {
                cx.write_to_clipboard(ClipboardItem::new_image(&picture));
            });
        if copied.is_none() {
            cx.write_to_clipboard(ClipboardItem::new_string(image.path.clone()));
            self.toast("Copied the image's path", cx);
        }
    }

    fn toast(&mut self, message: &str, cx: &mut Context<Self>) {
        let message = message.to_string();
        self.state.update(cx, |state, cx| {
            state.push_toast(message, ToastTone::Danger);
            cx.notify();
        });
    }
}

impl Render for ImageViewerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.opacity.set_target(if self.open { 1.0 } else { 0.0 });
        let dt = self.animator.dt(window, !self.opacity.settled());
        if dt > 0.0 {
            self.opacity.step(dt);
        }
        let t = self.opacity.value();
        if !self.open && t < 0.01 {
            return div().size_0();
        }
        let Some(image) = self.image.clone() else {
            return div().size_0();
        };
        let entity = cx.entity();
        let closer = entity.clone();
        let reveal = entity.clone();
        let copy = entity.clone();
        let viewer = Viewer::new(
            "transcript-image-viewer",
            image.title.clone(),
            ImageData::from_path(image.path.clone()),
        )
        .natural(image.width as f32, image.height as f32)
        .caption(image.caption.clone())
        .action(ImageAction::new(
            "image-reveal",
            Icon::Folder,
            "Reveal in Finder",
            move |_, cx| {
                reveal.update(cx, |view, cx| view.reveal(cx));
            },
        ))
        .action(ImageAction::new(
            "image-copy",
            Icon::Copy,
            "Copy",
            move |_, cx| {
                copy.update(cx, |view, cx| view.copy(cx));
            },
        ))
        .presence(t)
        .interactive(self.open)
        .on_close(move |_, cx| {
            closer.update(cx, |view, cx| view.close(cx));
        });
        // Escape closes the viewer wherever the focus is.
        let escape = self.open.then(|| {
            let entity = entity.clone();
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let entity = entity.clone();
                    window.on_key_event(move |event: &KeyDownEvent, phase, _, cx| {
                        if phase == DispatchPhase::Bubble && event.keystroke.key == "escape" {
                            entity.update(cx, |view, cx| view.close(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });
        // The control draws its own modal layer: a deferred element may not open inside
        // another, so the app never wraps it again.
        div().size_0().children(escape).child(viewer)
    }
}
