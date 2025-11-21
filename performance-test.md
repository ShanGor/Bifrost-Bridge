# Performance Test Commands and Report

```bash
hey -m POST -T "text/plain" -d "hello world, what they fuk you are doing now? i have no idea ffmpeg -i input.mp4 -i output.mp3 -map 0:v -map 1:a -c:v copy -c:a aac output.mp4" -c 50 -n 100000 -q 1000 -x http://localhost:3128  http://localhost:3030/echo.size
```


## 100k request to the same static file comparing Nginx;

### Nginx Results
```sh
>hey -m GET -c 100 -n 100000 -q 1000 http://localhost/ui/index.html

Summary:
Total:        14.4650 secs
Slowest:      0.3547 secs
Fastest:      0.0003 secs
Average:      0.0143 secs
Requests/sec: 6913.2219


Response time histogram:
0.000 [1]     |
0.036 [97948] |■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
0.071 [395]   |
0.107 [2]     |
0.142 [0]     |
0.178 [0]     |
0.213 [0]     |
0.248 [0]     |
0.284 [0]     |
0.319 [1108]  |
0.355 [490]   |


Latency distribution:
10% in 0.0022 secs
25% in 0.0050 secs
50% in 0.0090 secs
75% in 0.0126 secs
90% in 0.0177 secs
95% in 0.0225 secs
99% in 0.3122 secs

Details (average, fastest, slowest):
DNS+dialup:   0.0014 secs, 0.0003 secs, 0.3547 secs
DNS-lookup:   0.0029 secs, 0.0000 secs, 0.0460 secs
req write:    0.0010 secs, 0.0000 secs, 0.0420 secs
resp wait:    0.0020 secs, 0.0002 secs, 0.0386 secs
resp read:    0.0029 secs, 0.0000 secs, 0.0531 secs

Status code distribution:
[200] 99944 responses

Error distribution:
[56]  Get "http://localhost/ui/index.html": dial tcp: lookup localhost: context canceled

```

### Bifrost Bridge results
```sh
>hey -m GET -c 100 -n 100000 -q 1000 http://localhost:8088/ui/index.html

Summary:
Total:        5.8797 secs
Slowest:      0.3714 secs
Fastest:      0.0005 secs
Average:      0.0058 secs
Requests/sec: 17007.5441

Total data:   112300000 bytes
Size/request: 1123 bytes

Response time histogram:
0.000 [1]     |
0.038 [99899] |■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
0.075 [0]     |
0.112 [0]     |
0.149 [0]     |
0.186 [0]     |
0.223 [0]     |
0.260 [0]     |
0.297 [0]     |
0.334 [0]     |
0.371 [100]   |


Latency distribution:
10% in 0.0025 secs
25% in 0.0037 secs
50% in 0.0052 secs
75% in 0.0069 secs
90% in 0.0087 secs
95% in 0.0099 secs
99% in 0.0125 secs

Details (average, fastest, slowest):
DNS+dialup:   0.0003 secs, 0.0005 secs, 0.3714 secs
DNS-lookup:   0.0000 secs, 0.0000 secs, 0.0155 secs
req write:    0.0000 secs, 0.0000 secs, 0.0075 secs
resp wait:    0.0051 secs, 0.0003 secs, 0.0626 secs
resp read:    0.0003 secs, 0.0000 secs, 0.0098 secs

Status code distribution:
[200] 100000 responses
```
