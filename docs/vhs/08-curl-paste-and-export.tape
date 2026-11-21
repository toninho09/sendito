Output docs/vhs/out/08-curl-paste-and-export.gif
Set Shell "bash"
Set FontSize 15
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set TypingSpeed 60ms
Hide
Type "export SENDITO_KEYCHAIN=memory && cd /tmp/sendito-demo && clear" Enter
Show
Type "sendito" Enter
Sleep 1s
Ctrl+N Type `curl -X POST 'http://localhost:8787/items?x=1' -H 'X-Token: abc' --data-raw '{"a":1}'` Sleep 1s F5 Sleep 1500ms
Ctrl+W Type "u" Sleep 2s Escape
Ctrl+Q
