# Wishpad cat family

These original project assets form one small character family for the local “猫猫应愿” companion:

- `app-cat.png` / `app-cat.ico`: simplified cat face and pale-blue wish sparkle for the title bar and taskbar.
- `listening-cat.png` / `listening-cat.ico`: attentive cat used by the empty state.
- `holding-wish-cat.png` / `holding-wish-cat.ico`: cat safely holding a wish sparkle, shown in the short, nonactivating IME receipt after a wish is saved successfully.
- `organizing-cat.png` / `organizing-cat.ico`: cat holding a note, used by the record-organizing window.

The family keeps the same warm cream body, apricot tabby markings, dark cocoa outlines and pale sky-blue wish accent. Each transparent PNG is a 512 px project asset. Each ICO contains 16, 20, 24, 32, 40, 48, 64, 128 and 256 px frames. `build.rs` embeds resources 101–104 from `wishpad.rc` in `wishpad.exe`; the same bounded resource table is linked into the development TSF DLL so its successful wish receipt can draw resource 103 without file I/O.

The source illustrations were generated on 2026-08-03 with Codex's built-in image-generation tool. The existing `meditating-cat.png` was used only as the initial character-identity reference. Each new action was generated separately on a flat green chroma-key background, removed locally with the installed image-generation helper using a soft matte and despill, then cropped and downscaled with alpha preserved.

The design prompts required an original emoji-inspired flat illustration, rounded silhouette, consistent character proportions and palette, no text or watermark, and one action per asset: listening beside a drifting sparkle, holding a received sparkle, organizing a blank note, or a simplified head-only app mark.

`meditating-cat.png` and `meditating-cat.ico` remain as the earlier project asset and are no longer embedded by the current resource script. All assets in this directory are distributed under the repository's MPL-2.0 license.
