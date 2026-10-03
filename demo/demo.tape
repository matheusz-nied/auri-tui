# From the project root: vhs demo/demo.tape
# setup.sh builds auri and prepares a disposable repo with isolated preferences.
Output demo/demo.gif
Set Shell "bash"
Set FontSize 16
Set Width 1400
Set Height 800
Set Padding 16
Set Theme { "name": "auri", "background": "#100f0d", "foreground": "#e6e1d6" }
Set TypingSpeed 70ms
Set CursorBlink false
Set Framerate 10

Hide
Type@1ms "source demo/setup.sh" Enter
Wait+Screen@2m /AURI_DEMO_READY/
Type@1ms "clear" Enter
Sleep 300ms
Show
Sleep 800ms
Type@180ms "auri"
Sleep 500ms
Enter
Sleep 1s

# Explorer: expand src and open syntax-highlighted Rust code.
Type "l"
Sleep 500ms
Type "j" Enter
Sleep 2s
Type "2"
Type "jjjjjj"
Sleep 1s

# Leave the commit input before using navigation keys.
Type "c"
Escape
Sleep 300ms
Type "1"
Sleep 1s
Type "j" Enter
Sleep 1s
Type "n"
Sleep 2s
Type "n"
Sleep 1s

# Full-width diff, then restore the compact sidebar.
Type "b"
Sleep 2s
Type "b"
Sleep 1s

# Stage and unstage the same change, then stage the remaining files.
Type "1"
Space
Sleep 1s
Space
Sleep 1s
Type "a"
Sleep 1s

# A multi-line commit, only in the disposable demo repository.
Type "c"
Type "feat: refine the sky report"
Ctrl+j
Type "Filter stars and show constellations."
Sleep 2s
Enter
Sleep 2s
Escape
Sleep 300ms

# History: expand the new commit, move onto a file and show its diff.
Type "3g" Enter
Sleep 600ms
Type "jj" Enter
Sleep 2s
Type "n"
Sleep 2s

# Show built-in help and return to the diff for the closing frame.
Type "?"
Sleep 2s
Escape
Sleep 1s
Hide
Ctrl+c
Sleep 500ms
Type@1ms "exit" Enter
