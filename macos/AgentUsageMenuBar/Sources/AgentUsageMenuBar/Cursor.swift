import AppKit
import SwiftUI

/// Show the pointing-hand cursor while the pointer is over a view.
///
/// AppKit gives a plain arrow over every control, which is fine for things that already look like
/// buttons — but this app's clickable surfaces mostly don't. A ring gauge, a radio row and a
/// borderless glyph read as content until the cursor says otherwise, so the pointer is the only
/// affordance they have.
///
/// Uses `set()` rather than `push()`/`pop()`: a push that never gets its matching pop (the view
/// scrolls out from under the pointer, the popover closes mid-hover) leaves the wrong cursor stuck
/// for the whole app, where a stale `set` is corrected by the next cursor rect the pointer enters.
private struct PointingHandCursor: ViewModifier {
    func body(content: Content) -> some View {
        content.onHover { inside in
            if inside {
                NSCursor.pointingHand.set()
            } else {
                NSCursor.arrow.set()
            }
        }
    }
}

extension View {
    /// Mark this view as clickable to the pointer. See [`PointingHandCursor`].
    func pointingHandCursor() -> some View {
        modifier(PointingHandCursor())
    }
}
