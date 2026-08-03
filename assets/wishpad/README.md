# Wishpad icon

`meditating-cat.png` and `meditating-cat.ico` are original project assets generated for the
local “向猫猫许愿” companion. The cat closes its eyes and rests its paws in a meditation pose;
the small blue sparkle represents a wish being received.

The source illustration was generated on 2026-08-02 with Codex's built-in image generation
tool from this prompt:

> Create an original, emoji-inspired little cat meditating peacefully with eyes closed, as if
> quietly receiving and holding the user's wish. Use a compact rounded silhouette, warm cream
> and soft apricot colors, dark cocoa facial lines, and one tiny pale sky-blue sparkle. Keep it
> centered, symmetric, readable at 16 px, without text, watermark, badge, app tile or copied
> platform-emoji design. Place it on a flat green chroma-key background for local removal.

The background was removed locally with the installed image-generation skill helper using a
soft matte, despill and one-pixel edge contraction. The final ICO contains 16, 20, 24, 32, 40,
48, 64 and 256 px frames. `build.rs` embeds resource 101 into `wishpad.exe`; the same resource
is used by the title bar and notification-area icon. These assets are distributed under the
repository's MPL-2.0 license.
