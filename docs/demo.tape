# Record the README demo:  vhs docs/demo.tape
#
# Needs vhs (brew install vhs) and a release build in place.
# The run is sent to a scratch config and cache under target/demo, so it
# leaves the playlists, history and audio cache of a real install alone.
# The volume is taken to zero before recording starts: the player really
# plays while vhs records, and a demo should not make noise.

Output docs/demo.gif

Set Shell "zsh"
Set FontSize 16
Set Width 1040
Set Height 640
Set Padding 20
Set Framerate 12
Set TypingSpeed 70ms

Hide
Type "export HARIOTO_CACHE_DIR=$PWD/target/demo/cache XDG_CONFIG_HOME=$PWD/target/demo/config && clear"
Enter
Sleep 1s
Type "./target/release/harioto --lang en"
Enter
Sleep 6s
Type "---------------"
Sleep 1200ms
Show

Sleep 1500ms
Type "/"
Sleep 700ms
Type "統神歌劇院"
Sleep 900ms
Enter
Sleep 8s
Enter
Sleep 6s
Type "r"
Sleep 7s
Down
Sleep 500ms
Down
Sleep 500ms
Down
Sleep 1500ms
Type "g"
Sleep 2s
Type "g"
Sleep 2500ms
