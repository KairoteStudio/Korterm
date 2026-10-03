// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
//! Cubic-bezier easing used by the dropdown expand animation.
//!
//! Mirrors CSS `cubic-bezier(x1, y1, x2, y2)` timing functions.

/// Duration of the dropdown expand animation, in milliseconds.
pub const MENU_ANIM_MS: f32 = 240.0;

/// Evaluate a CSS-style cubic bezier at progress `x` (0..1) and return the
/// eased value `y` (0..1). Uses Newton-Raphson with a bisection fallback so
/// it converges for every control-point combination.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }

    let cx = 3.0 * x1;
    let bx = 3.0 * (x2 - x1) - cx;
    let ax = 1.0 - cx - bx;
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;

    let sample_x = |t: f32| ((ax * t + bx) * t + cx) * t;
    let sample_y = |t: f32| ((ay * t + by) * t + cy) * t;
    let sample_dx = |t: f32| (3.0 * ax * t + 2.0 * bx) * t + cx;

    // Newton-Raphson refinement.
    let mut t = x;
    for _ in 0..8 {
        let err = sample_x(t) - x;
        if err.abs() < 1e-6 {
            break;
        }
        let d = sample_dx(t);
        if d.abs() < 1e-6 {
            break;
        }
        t -= err / d;
    }
    t = t.clamp(0.0, 1.0);

    // Bisection fallback if Newton did not converge tightly.
    if (sample_x(t) - x).abs() > 1e-4 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = (lo + hi) * 0.5;
            if sample_x(mid) < x {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        t = (lo + hi) * 0.5;
    }

    sample_y(t)
}

/// The easing curve for dropdown expansion — an aggressive ease-out
/// (`cubic-bezier(0.16, 1, 0.3, 1)`, "ease-out-expo"-like): the menu
/// "explodes" out of the button and decelerates into place.
pub fn menu_ease(t: f32) -> f32 {
    cubic_bezier(0.16, 1.0, 0.3, 1.0, t.clamp(0.0, 1.0))
}

/// Ease-in curve for menu close (mirror of the open ease).
pub fn menu_close_ease(t: f32) -> f32 {
    cubic_bezier(0.7, 0.0, 0.84, 0.0, t.clamp(0.0, 1.0))
}

/// Duration of the dropdown close animation, in milliseconds.
pub const MENU_CLOSE_MS: f32 = 140.0;

// =============================================================================
// FLIP tween widget — slide a widget from its old layout position to the new
// =============================================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use iced::advanced::layout;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{Clipboard, Layout, Shell, mouse};
use iced::{Element, Rectangle, Size, Vector};

/// Duration of the tab swap slide, in milliseconds.
pub const TAB_TWEEN_MS: f32 = 160.0;

pub fn ease_out_cubic(t: f32) -> f32 {
    let u = 1.0 - t;
    1.0 - u * u * u
}

/// One tracked widget: its last layout bounds plus an in-flight tween.
#[derive(Clone, Copy)]
struct Track {
    bounds: Rectangle,
    anim: Option<(Vector, Instant)>,
    seen: Instant,
}

/// Position tracker shared by every [`Tween`] of a tab bar.
///
/// Keyed by a **stable widget id** (not the row slot!) so a tab slides from
/// its own previous position to its new one when slots swap — the previous
/// per-slot approach only caught width differences and looked like a jump.
///
/// Owned by the app state (`Rc`-shared into the widgets each frame).
#[derive(Default)]
pub struct TweenTracker {
    tabs: HashMap<u64, Track>,
}

impl TweenTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed a tab's position so its first frame tweens from `at`
    /// (e.g. under the "+" button) to its real slot, instead of popping in.
    pub fn seed(&mut self, key: u64, at: (f32, f32)) {
        self.tabs.insert(
            key,
            Track {
                bounds: Rectangle {
                    x: at.0,
                    y: at.1,
                    width: 0.0,
                    height: 0.0,
                },
                anim: None,
                seen: Instant::now(),
            },
        );
    }

    /// Seed a tab's position only when the tracker has none — used before
    /// a layout switch so never-laid-out (overflow) tabs still FLIP in.
    pub fn seed_missing(&mut self, key: u64, at: (f32, f32)) {
        self.tabs.entry(key).or_insert_with(|| Track {
            bounds: Rectangle {
                x: at.0,
                y: at.1,
                width: 0.0,
                height: 0.0,
            },
            anim: None,
            seen: Instant::now(),
        });
    }

    /// Last known layout position of a tab (window coordinates), used to
    /// anchor popups (e.g. the tab context menu) to the tab itself.
    pub fn below_of(&self, key: u64) -> Option<(f32, f32)> {
        self.tabs
            .get(&key)
            .map(|t| (t.bounds.x, t.bounds.y + t.bounds.height + 2.0))
    }
}

/// Shared handle passed to every `Tween` of the same tab bar.
pub type SharedTweenTracker = Rc<RefCell<TweenTracker>>;

/// Wraps any widget and animates it sliding from its previous layout
/// position to the current one (FLIP: capture start, layout to end, tween).
///
/// Retargeting is continuous: if the position changes again while a tween is
/// running, the current visual offset is folded into the new tween, so the
/// widget never jumps mid-flight. Hit-testing stays on the real (final)
/// layout bounds, so interaction remains stable during the slide.
pub struct Tween<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    key: u64,
    tracker: SharedTweenTracker,
    content: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> Tween<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    pub fn new(
        key: u64,
        tracker: &SharedTweenTracker,
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Tween {
            key,
            tracker: Rc::clone(tracker),
            content: content.into(),
        }
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Tween<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<iced::Length> {
        self.content.as_widget().size()
    }

    // Transparent-wrapper tree passthrough: the child owns the tree state.
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(tree, renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            tree,
            layout,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let mut translation = Vector::default();

        {
            let mut map = self.tracker.borrow_mut();
            let now = Instant::now();
            let track = map.tabs.entry(self.key).or_insert(Track {
                bounds,
                anim: None,
                seen: now,
            });

            // Where is the widget visually right now (bounds + in-flight
            // tween offset)? That is the tween's start point.
            let mut visual = track.bounds;
            if let Some((d, start)) = &mut track.anim {
                let p = ((start.elapsed().as_secs_f32() * 1000.0)
                    / TAB_TWEEN_MS)
                    .min(1.0);
                let e = ease_out_cubic(p);
                let t = Vector::new(d.x * (1.0 - e), d.y * (1.0 - e));
                visual = Rectangle {
                    x: track.bounds.x + t.x,
                    y: track.bounds.y + t.y,
                    ..track.bounds
                };
                if p >= 1.0 {
                    track.anim = None;
                    visual = track.bounds;
                }
            }

            // Moved since last frame? Start / retarget the tween from the
            // current visual position to the new layout bounds.
            let dx = visual.x - bounds.x;
            let dy = visual.y - bounds.y;
            if dx.abs() > 0.5 || dy.abs() > 0.5 {
                track.anim = Some((Vector::new(dx, dy), Instant::now()));
                translation = Vector::new(dx, dy);
            } else if let Some((d, start)) = &track.anim {
                let p = ((start.elapsed().as_secs_f32() * 1000.0)
                    / TAB_TWEEN_MS)
                    .min(1.0);
                let e = ease_out_cubic(p);
                translation = Vector::new(d.x * (1.0 - e), d.y * (1.0 - e));
            }

            track.bounds = bounds;
            track.seen = now;

            // GC stale entries (closed tabs) every once in a while.
            if map.tabs.len() > 64 {
                map.tabs
                    .retain(|_, t| t.seen.elapsed().as_secs() < 5);
            }
        }

        let child_layout = layout;
        let content = self.content.as_widget();
        let child_tree = tree;

        if translation.x != 0.0 || translation.y != 0.0 {
            renderer.with_translation(translation, |renderer| {
                content.draw(
                    child_tree, renderer, theme, style, child_layout, cursor,
                    viewport,
                );
            });
        } else {
            content.draw(
                child_tree, renderer, theme, style, child_layout, cursor,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<Tween<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(tween: Tween<'a, Message, Theme, Renderer>) -> Self {
        Element::new(tween)
    }
}

// =============================================================================
// Shifted widget — draw content at a fixed offset (for the terminal
// content switch slide)
// =============================================================================

/// Draws its content translated by a fixed `offset` (no state, no tracking).
/// The app recomputes the offset every frame while the switch animation
/// runs, so this stays dumb on purpose — the per-key tweening lives in
/// [`Tween`].
pub struct Shifted<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    offset: Vector,
    content: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> Shifted<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    pub fn new(
        offset: Vector,
        content: impl Into<Element<'a, Message, Theme, Renderer>>,
    ) -> Self {
        Shifted {
            offset,
            content: content.into(),
        }
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Shifted<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<iced::Length> {
        self.content.as_widget().size()
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(tree, renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let content = self.content.as_widget();

        if self.offset.x != 0.0 || self.offset.y != 0.0 {
            renderer.with_translation(self.offset, |renderer| {
                content.draw(
                    tree, renderer, theme, style, layout, cursor, viewport,
                );
            });
        } else {
            content.draw(
                tree, renderer, theme, style, layout, cursor, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<Shifted<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(shifted: Shifted<'a, Message, Theme, Renderer>) -> Self {
        Element::new(shifted)
    }
}

// =============================================================================
// ImeZone widget — declares an input-method region (enables IME over the
// terminal canvas)
// =============================================================================

/// Wraps any widget and tells the windowing system that an input method
/// (e.g. fcitx5/ibus) is wanted over this zone. Without it, iced keeps
/// `set_ime_allowed(false)` for non-text widgets and IME commits never
/// arrive — which is why Chinese input did not work inside the terminals.
///
/// Committed text is delivered as `Event::InputMethod(Commit(..))` via the
/// global event subscription; this widget only handles activation.
pub struct ImeZone<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    content: Element<'a, Message, Theme, Renderer>,
    enabled: bool,
    preedit: String,
    cursor: Option<Rectangle>,
}

impl<'a, Message, Theme, Renderer> ImeZone<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    pub fn new(content: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        ImeZone {
            content: content.into(),
            enabled: true,
            preedit: String::new(),
            cursor: None,
        }
    }

    /// Configure the IME strategy for this frame:
    /// - `enabled`: keep the input method active (disable while a
    ///   modifier-only shortcut like Ctrl+` is held, or the IME layer
    ///   swallows the key).
    /// - `preedit`: composition text drawn over-the-spot by the runtime.
    /// - `cursor`: anchor rectangle for the IME candidate window.
    pub fn ime(
        mut self,
        enabled: bool,
        preedit: String,
        cursor: Option<Rectangle>,
    ) -> Self {
        self.enabled = enabled;
        self.preedit = preedit;
        self.cursor = cursor;
        self
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ImeZone<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn size(&self) -> Size<iced::Length> {
        self.content.as_widget().size()
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(tree, renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Keep the IME enabled over this zone (anchored near its top-left,
        // where the shell prompt / cursor usually sits).
        let bounds = layout.bounds();
        if self.enabled {
            let cursor = self.cursor.unwrap_or(Rectangle {
                x: bounds.x + 6.0,
                y: bounds.y + 6.0,
                width: 1.0,
                height: 1.0,
            });
            shell.request_input_method(
                &iced::advanced::input_method::InputMethod::<String>::Enabled {
                    cursor,
                    purpose: iced::advanced::input_method::Purpose::Terminal,
                    preedit: if self.preedit.is_empty() {
                        None
                    } else {
                        Some(iced::advanced::input_method::Preedit {
                            content: self.preedit.clone(),
                            selection: None,
                            text_size: Some(iced::Pixels(13.0)),
                        })
                    },
                },
            );
        }

        self.content.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            tree,
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            tree,
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<ImeZone<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: renderer::Renderer + 'a,
{
    fn from(zone: ImeZone<'a, Message, Theme, Renderer>) -> Self {
        Element::new(zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_endpoints_are_exact() {
        assert_eq!(menu_ease(0.0), 0.0);
        assert_eq!(menu_ease(1.0), 1.0);
        assert_eq!(menu_close_ease(-0.5), 0.0);
        assert_eq!(menu_close_ease(1.5), 1.0);
    }

    #[test]
    fn menu_ease_is_monotonic_and_bounded() {
        let mut prev = 0.0_f32;
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let y = menu_ease(t);
            assert!(
                y >= prev - 1e-5,
                "menu_ease regressed at t={t}: {prev} -> {y}"
            );
            assert!((0.0..=1.0).contains(&y), "menu_ease out of range at t={t}");
            prev = y;
        }
    }

    #[test]
    fn menu_ease_is_ease_out() {
        // Ease-out-expo-like: halfway through, the motion is mostly done.
        assert!(menu_ease(0.5) > 0.8, "menu_ease(0.5) = {}", menu_ease(0.5));
    }

    #[test]
    fn menu_close_ease_stays_bounded() {
        for i in 0..=50 {
            let y = menu_close_ease(i as f32 / 50.0);
            assert!((0.0..=1.0).contains(&y));
        }
    }

    #[test]
    fn ease_out_cubic_known_values() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        assert_eq!(ease_out_cubic(1.0), 1.0);
    }
}
