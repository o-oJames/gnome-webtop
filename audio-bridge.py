#!/usr/bin/env python3
"""
WebSocket audio bridge: streams PulseAudio monitor → browser.
Browser connects via WebSocket and receives raw PCM chunks for playback.
"""
import asyncio
import signal
import sys

try:
    import websockets
except ImportError:
    print("❌ python3-websockets not installed", file=sys.stderr)
    sys.exit(1)

WS_HOST = "0.0.0.0"
WS_PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 6902

# PCM format: 16-bit signed LE, 44100 Hz, stereo
RATE = 44100
CHANNELS = 2
FORMAT = "s16le"
CHUNK_MS = 50  # send a chunk every 50ms
CHUNK_SIZE = int(RATE * CHANNELS * 2 * CHUNK_MS / 1000)  # bytes per chunk

clients: set = set()


async def pulseaudio_reader():
    """Read raw PCM from PulseAudio monitor and broadcast to WebSocket clients."""
    cmd = [
        "parecord",
        "--device=webtop_output.monitor",
        "--format", FORMAT,
        "--rate", str(RATE),
        "--channels", str(CHANNELS),
        "--raw",
    ]
    while True:
        proc = None
        try:
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.DEVNULL,
            )
            print(f"🎵 parecord started (PID {proc.pid})", flush=True)
            while True:
                data = await proc.stdout.read(CHUNK_SIZE)
                if not data:
                    break
                if clients:
                    dead = []
                    for ws in list(clients):
                        try:
                            await ws.send(data)
                        except Exception:
                            dead.append(ws)
                    # Remove dead clients without rebinding the set
                    for ws in dead:
                        clients.discard(ws)
        except Exception as e:
            print(f"⚠️  parecord error: {e}, retrying in 2s...", flush=True)
            await asyncio.sleep(2)
        finally:
            if proc and proc.returncode is None:
                proc.kill()
                await proc.wait()


async def ws_handler(websocket):
    """Handle a browser WebSocket connection."""
    clients.add(websocket)
    addr = websocket.remote_address
    print(f"🔊 Audio client connected: {addr} ({len(clients)} total)", flush=True)
    try:
        async for _ in websocket:
            pass  # ignore incoming messages
    finally:
        clients.discard(websocket)
        print(f"🔇 Audio client disconnected: {addr} ({len(clients)} total)", flush=True)


async def main():
    async with websockets.serve(ws_handler, WS_HOST, WS_PORT):
        print(f"✅ Audio WebSocket bridge on ws://0.0.0.0:{WS_PORT}", flush=True)
        await pulseaudio_reader()


if __name__ == "__main__":
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, loop.stop)
    try:
        loop.run_until_complete(main())
    except KeyboardInterrupt:
        pass
