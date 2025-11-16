# Performance Test Commands and Report

```bash
hey -m POST -T "text/plain" -d "hello world, what they fuk you are doing now? i have no idea ffmpeg -i input.mp4 -i output.mp3 -map 0:v -map 1:a -c:v copy -c:a aac output.mp4" -c 5 -n 100000 -q 1000 -x http://localhost:3128  http://localhost:8080/test
```
