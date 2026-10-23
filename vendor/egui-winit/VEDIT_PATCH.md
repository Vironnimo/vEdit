# vEdit's clipboard input provenance patch

This is the crates.io source of `egui-winit 0.36.2`, already selected by vEdit's
lockfile, with a small input translation patch. Its original source commit is
`49682f8baa058bf49e011035cfbd6e825f88a5ef` (`crates/egui-winit`), recorded in
`.cargo_vcs_info.json`; the published crate checksum is
`98466000559d66ba4db786a30448b01483946add4188e7f10f522a27b4fb89cc`.
`LICENSE-MIT` and `LICENSE-APACHE` are copied from that upstream commit.
The root `[patch.crates-io]` selects this copy without upgrading dependencies.

Upstream `State::on_keyboard_input` returns after emitting Copy, Cut or Paste,
before emitting the originating Key. Paste with an empty or unavailable clipboard
emits nothing. An application cannot distinguish Ctrl+V from Shift+Insert using
those semantic events, and eframe's `raw_input_hook` runs after this loss.
Configurable forwarding of individual Ctrl chords to Vim therefore needs the
original key as well as the native clipboard event.

## Event contract

`src/clipboard_input.rs::push_key` is an internal helper called by the existing
keyboard translator. It emits the logical Key (with existing physical-key
metadata and modifiers) first, immediately followed by its semantic clipboard
event when applicable. Empty or unavailable paste still emits the originating
Key. The clipboard-command result continues to suppress subsequent text events.

The existing shortcut predicates are unchanged: platform command+C/X/V,
dedicated Copy/Cut/Paste keys, and the Windows Ctrl+Insert, Shift+Insert and
Shift+Delete alternatives. Logical-or-physical fallback, key release handling,
egui's repeat detection, clipboard reads, CRLF normalization and empty-paste
filtering retain upstream behavior. No public API is added.

An application that handles an originating key itself must consume its adjacent
matching clipboard event too. vEdit forwards exact configured Ctrl+C/X/V chords
to Vim and removes only that pair; native alternatives and standalone semantic
events keep their clipboard meaning. Native editor and multicursor handlers
deduplicate raw clipboard keys against semantic events. Standard egui TextEdit
handles clipboard semantics and already guards Windows Shift+Delete against an
additional raw-key deletion.

## Verification and maintenance

`src/main.rs` includes this exact production helper in vEdit's normal test suite:
`cargo test --bin vedit native_clipboard_input_tests`. The tests cover key and
semantic event order, physical provenance, dedicated and Windows alternative
keys, empty/unavailable paste, CRLF normalization, releases and lazy clipboard
reads. App input-routing tests cover native/Vim ownership and editor modes.
These deterministic tests do not send physical keys or access the OS clipboard;
a native desktop check remains necessary when replacing the input backend.

`scripts/build-release.ps1` ships both licenses and this patch note under
`licenses/egui-winit/`. The crate source and original normalized manifest remain
in the repository; only the translator and new helper differ from upstream.
`Cargo.toml.orig` records the upstream workspace manifest for provenance.

When upgrading egui/eframe, review clipboard event ordering and TextEdit's native
alternatives, then run both backend and app routing tests. Remove this patch only
when upstream preserves equivalent origin information, including empty paste,
and vEdit's configured Ctrl handling works with native Windows clipboard input.
