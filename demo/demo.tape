# Record the README/landing demo:  vhs demo/demo.tape
# Requires https://github.com/charmbracelet/vhs. Run from a repo with a few
# uncommitted changes so the Source Control view has something to show.

Output demo/demo.gif
Set Shell "bash"
Set FontSize 15
Set Width 1400
Set Height 800
Set Theme "OneHalfDark"
Set TypingSpeed 60ms

Hide
Type "cargo build --release -q && clear" Enter
Sleep 3s
Show

Type "./target/release/auri" Enter
Sleep 2s

# Explorer: open a file
Type "e"
Sleep 800ms
Type "j" Sleep 300ms Type "l" Sleep 500ms
Type "j" Sleep 300ms Type "j" Sleep 300ms Enter
Sleep 1.5s
Type "2"
Type "jjjjjjjj"
Sleep 1s

# Source Control: look at a diff
Type "c"
Sleep 600ms
Type "1"
Sleep 400ms
Enter
Sleep 1s
Type "n"
Sleep 1s
Type "n"
Sleep 1.5s

# History
Type "3"
Sleep 600ms
Type "j" Sleep 400ms Enter
Sleep 800ms
Enter
Sleep 2s

Type "q"
Sleep 500ms
