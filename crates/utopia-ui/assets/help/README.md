# Help screenshots

These PNGs are embedded in the binary by `crates/utopia-ui/src/help.rs` and shown in
Settings → Help. Regenerate them with:

```bash
cargo run -p xtask -- help-shots
```

That builds the app, seeds a mock graph in `target/help-shots/Demo.utopia` (a small
project, a person and a few days of notes that link them), drives the real window with
`UTOPIA_SHOT_SCRIPT` and downscales each capture to 1200 px wide.

The app captures its own window, so no Screen Recording permission is needed. The Settings
window is a separate window; the script reaches it with `target last` (after `keys cmd-,`),
which is how the Assistants topic gets a picture of Settings → Assistants. The app is run
from a bundle-shaped copy at `target/help-shots/Utopia.app/Contents/MacOS/utopia`, so the
client entries in that picture do not name `target/debug`.

Every capture is downscaled to 1200 px wide; `help.rs` carries each one's height ratio, so a
Settings capture (1200 × 864) is not squeezed into the main window's shape (1200 × 798).

Review the captures by eye after regenerating: colours, typography and layout are not
covered by tests (see AGENTS.md, "Known limits").
