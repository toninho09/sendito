| Measurement | Mean | p99 | Max | Target | Result |
|---|---|---|---|---|---|
| Cold start → first frame (in-process, 20 runs) | 3.0 ms | 3.8 ms | 3.8 ms | ≤ 100 ms | PASS |
| Cold start, end to end through a pty wrapper (hyperfine, 20 runs; the wrapper alone takes 20.9 ms) | 21.0 ms | 21.4 ms | 21.4 ms | ≤ 100 ms | PASS |
| Max RSS, idle with the 1,000-request collection | 12.4 MB | | | ≤ 30 MB | PASS |
| Max RSS, showing a 10 MB JSON response | 47.8 MB | | | ≤ 120 MB | PASS |
| Frame render during 60 s scroll/type (1355 frames) | 0.18 ms | 0.59 ms | 2.26 ms | p99 ≤ 16 ms | PASS |
| Longest event / async-message handling on the UI thread (1353) | 0.01 ms | 0.04 ms | 0.53 ms | ≤ 50 ms | PASS |
