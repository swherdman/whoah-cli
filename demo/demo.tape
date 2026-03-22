# WHOAH CLI — Demo Recording
#
# Run with: vhs demo/demo.tape
# Requires: whoah --demo (simulated pipeline, no SSH)

# ── Terminal settings ──────────────────────────────────────────
Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 800
Set Theme "Dracula"
Set TypingSpeed 60ms
Set PlaybackSpeed 1.0
Set Framerate 15
Set Padding 0
Set BorderRadius 8

# ── Output ─────────────────────────────────────────────────────
Output demo/whoah-demo.gif

# ── Launch app off-screen ──────────────────────────────────────
Hide
Type "cargo run -- --demo"
Enter
Sleep 3s
Show

# ── Config screen ──────────────────────────────────────────────
Sleep 1s

# Focus right panel and browse config tabs
Type "l"
Sleep 1s
Right
Sleep 800ms
Right
Sleep 800ms
Right
Sleep 800ms
Right
Sleep 2s

# ── Build screen ───────────────────────────────────────────────
Type "2"
Sleep 1s

# Start the build pipeline
Type "b"

# Wait for the pipeline to finish (~25s)
Wait+Screen@60s /Complete/

Sleep 3s

# ── Monitor screen ─────────────────────────────────────────────
Type "3"
Sleep 12s
