Output docs/vhs/out/06-grpc-bidi-stream.gif
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
Ctrl+P Type ":newgrpc" Enter Type "localhost:50051" Tab Enter Sleep 1500ms Type "Echo/Bidi" Enter Sleep 1s
Alt+1 Enter Ctrl+A Type `{"text": "one"}` F5 Sleep 1s Ctrl+A Type `{"text": "two"}` F5 Sleep 1s
Ctrl+W Type "d" Sleep 1s Ctrl+W Type "j" Alt+4 Sleep 2s
Ctrl+Q
