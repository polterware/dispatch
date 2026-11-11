//! The voice blob beside the composer: a small disc that swells with the microphone or with
//! playback, drawn on a canvas so a level read each frame never lays the page out again.

use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, IntoElement, Pixels, Window, canvas, div, fill, point, px, size,
};

use crate::theme::Theme;
use crate::voice::{LevelSource, VoiceEngine};

/// The disc's box.
const BLOB: f32 = 22.0;

/// A disc for the microphone (`speaking` false) or for playback, breathing with `engine`'s
/// level. It asks for a frame each paint, so it settles the moment it is gone from the page.
pub fn voice_blob(engine: &VoiceEngine, speaking: bool, theme: &Theme) -> AnyElement {
    let engine = engine.clone();
    let color = if speaking {
        theme.color_accent
    } else {
        theme.color_brand
    };
    div()
        .flex_none()
        .size(px(BLOB))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<Pixels>, _, window: &mut Window, _: &mut App| {
                    paint(bounds, &engine, speaking, color, window);
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn paint(
    bounds: Bounds<Pixels>,
    engine: &VoiceEngine,
    speaking: bool,
    color: gpui::Rgba,
    window: &mut Window,
) {
    let source = if speaking {
        LevelSource::Output
    } else {
        LevelSource::Mic
    };
    let level = engine.level(source).clamp(0.0, 1.0);
    let max = f32::from(bounds.size.width) / 2.0;
    let radius = max * (0.4 + 0.55 * level);
    let center = bounds.center();
    let disc = Bounds::new(
        point(center.x - px(radius), center.y - px(radius)),
        size(px(radius * 2.0), px(radius * 2.0)),
    );
    let mut fill_color = color;
    fill_color.a = 0.3 + 0.55 * level;
    window.paint_quad(fill(disc, fill_color).corner_radii(px(radius)));
    window.request_animation_frame();
}
