import os, pty, sys, time, select, fcntl, termios, struct, pyte
cols, rows = 180, 40
pid, fd = pty.fork()
if pid == 0:
    os.chdir(os.path.expanduser("~/2026/y"))
    os.execvp(os.environ.get("ZO_BIN", os.path.expanduser("~/.local/bin/zo")), ["zo"])
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
screen = pyte.Screen(cols, rows); stream = pyte.ByteStream(screen)
buf = bytearray()
def pump(t):
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                data = os.read(fd, 65536)
            except OSError:
                return
            if not data:
                return
            buf.extend(data); stream.feed(data)
pump(5)
text = open(sys.argv[1], encoding="utf-8").read()
_unused = ("Play Console 요건을 충족할 수 있도록 2026년 9월 30일까지 앱과 서명 키를 등록하세요\n"
        "Google Play 앱이 자동 등록되었지만, Play Console 홈페이지에서 자동 등록되지 않은 앱은 Google Play에서 삭제될 예정입니다.\n"
        "또한 Play Console을 사용하여 Google Play 외부에서 배포하는 Android 앱과 Play 외부에서 서명하는 데 사용하는 Google Play 앱의 추가 키를 등록하실 수 있습니다. 각 앱 옆에 패키지 이름 상태가 이미 시작한 임시 패키지 이름 등록이 있는 경우 등록을 완료해 주세요. 자세히 알아보기\n"
        "2026년 9월 30일부터 다른 참여 스토어의 Android 앱에 대한 미등록된 패키지 이름과 서명 키 쌍은 일부 국가의 인증된 Android 기기에 설치할 수 없습니다.\n"
        "이거 자동화 등록해줘")
mark = len(buf)
os.write(fd, b"\x1b[200~" + text.encode() + b"\x1b[201~")
pump(3)
open(sys.argv[2], "wb").write(bytes(buf))
for i, line in enumerate(screen.display):
    if line.strip():
        print(f"{i:02d}|{line.rstrip()}|")
print("cursor", screen.cursor.x, screen.cursor.y)
os.write(fd, b"\x03"); time.sleep(0.3); os.write(fd, b"\x03"); pump(1)
try: os.kill(pid, 9)
except Exception: pass
