# vEdit's first-frame presentation patch

This is the crates.io source of eframe 0.36.2, from upstream commit
`49682f8baa058bf49e011035cfbd6e825f88a5ef` (`crates/eframe`). The original
MIT and Apache-2.0 licenses are included. The root `[patch.crates-io]` selects
this copy without changing the dependency version or adding a package.

The Glow backend creates the root window hidden, but upstream calls
`integration.post_rendering` before `gl_surface.swap_buffers`. That call shows
the window, exposing its unpresented front buffer while the first UI image is
still in the back buffer. Windows can briefly display a blank surface.

The patched Glow path calls `post_rendering` only after a successful buffer swap.
The first visible window therefore already has its rendered pixels. A failed
swap does not reveal the window. Later frames retain the existing one-time show
behavior; the WGPU path is unchanged.

`tests/startup_window.rs` exercises the actual eframe entry point with a private
storage path. A synchronous Windows subclass callback reads the OpenGL front
buffer when the window is first shown, before the native show handler runs.
It expects the painted dark surface, then closes only its own fixture window.
The test also covers maximized startup and observes native show requests before
the app constructor; the separate hidden-state fix is documented in
`vendor/winit/VEDIT_PATCH.md`.
Asynchronous screenshots can miss the interval this test covers.

Run on a Windows desktop with OpenGL:

```powershell
cargo test --test startup_window -- --ignored --nocapture
```

The test is ignored by default because it needs a native desktop and graphics
driver. Headless snapshots cannot verify this ordering. When upgrading eframe,
remove this patch only once upstream shows the Glow window after presenting its
first frame and this native regression passes against the replacement.
