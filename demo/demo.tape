Output demo.gif

Require cargo

Set Shell "bash"
Set FontSize 22
Set Width 1200
Set Height 700
Set Theme "Catppuccin Mocha"
Set WindowBar Colorful
Set WindowBarSize 40
Set TypingSpeed 40ms
Set Padding 20

# Pre-build and set up environment
Hide
Type "cd /Users/justinchapman/src/tryke" Enter
Sleep 200ms
Type "export PYTHONPATH=$(pwd)/python" Enter
Sleep 200ms
Type "cargo build 2>/dev/null" Enter
Sleep 10s
Type "clear" Enter
Sleep 500ms
Show

# Show the test file
Type "cat demo/tests/test_math.py" Sleep 500ms Enter
Sleep 3s

# Run tryke
Type "cargo run -- test --root demo" Sleep 500ms Enter
Sleep 5s
