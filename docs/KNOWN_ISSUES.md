# Known Issues

Known issues and workarounds in Connected.

## Non-Media Attachments Are Invisible in Threads

An MMS whose attachment is a PDF, document or other non-media file renders as an empty bubble. The file itself is sent and delivered correctly. This is a KDE Connect Android limitation, not an applet bug.

### Symptoms
- Send a PDF with no accompanying text produces a "Sending" bubble that is later joined by (or replaced with) an empty bubble carrying only a timestamp
- Received non-media attachments show the same empty bubble
- The conversation list preview for the thread is also empty, since there is no body and no attachment flag
- Images, videos and audio in the same thread render normally

### Technical details

This limitation exists in the KDE Connect Android app which only supports audio, text, image and video images. This affects every KDE Connect desktop client, not only Connected

## Some MMS Videos Fail to Play in COSMIC Player

Opening an MMS video attachment can launch COSMIC Player and then fail with an error suggesting the file is missing. The file is present and intact — this is a COSMIC Player / GStreamer limitation with iPhone-recorded video, not an applet bug.

**Upstream issue:** `pop-os/cosmic-player` — not yet filed (no duplicate as of 2026-07-18).

### Symptoms

- Tapping an MMS video attachment opens COSMIC Player, which reports the file cannot be found
- Only affects *some* videos — typically those sent from iPhones
- Images from the same conversation open normally
- The same file plays correctly in VLC (confirmed 2026-07-18)

### Technical Details

Connected resolves the attachment from the daemon cache and hands the path to `xdg-open`, which routes it to the system default handler (`Exec=cosmic-player %U`). The journal shows the file *is* reached — the URI resolves correctly, including the space and `+` in device-named cache directories like `Galaxy S24+`:

```
cosmic_player::video: failed to open
  file:///home/.../kdeconnect.daemon/Galaxy%20S24+/PART_1784405118130
  : invalid framerate: 0
cosmic_store: failed to load gstreamer codec "meta/x-gst-fourcc-mebx decoder"
cosmic_player: failed to install plugins: not-found
```

iPhone-recorded MP4s carry `mebx` tracks (Apple Metadata Event Box) alongside the audio and video streams. GStreamer has no handler for them, so COSMIC Player attempts to auto-install a `meta/x-gst-fourcc-mebx` decoder, fails, and then reads the frame rate from a metadata track (`0/0`) rather than the video track — producing `invalid framerate: 0`, which the UI surfaces as a file-not-found style error.

`ffprobe` on an affected file shows the video stream is perfectly valid:

```
stream 1:   h264, 30/1 fps          ← fine
stream 2-6: codec_tag=mebx, codec_name=unknown, r_frame_rate=0/0
```

Across a sample of six cached MMS videos, the four containing `mebx` tracks failed and the two without them played normally.

### Workaround

Set a different default video player. VLC handles these files correctly:

```bash
xdg-mime default vlc.desktop video/mp4
```

Nothing in Connected needs to change — the attachment is downloaded, cached, and dispatched correctly.

## Device Shows Connected After the Phone Silently Leaves the Network

When a phone leaves Wi-Fi abruptly (out of range, radio off - no clean disconnect), the daemon
keeps reporting the device as reachable and Connected mirrors that faithfully. Measured window:
up to ~16 minutes. Actions taken during it - sending an SMS in particular - can be silently
dropped.

**Not an applet bug.** `Device::isReachable()` upstream is "a link object exists", not "the link
works", and upstream documents its own blindness here. KDE Connect's TCP keepalive would notice
in ~25 s, but keepalive only runs on an *idle* socket; if a request was already in flight when
the phone vanished, the kernel's retransmission budget governs instead (~15 minutes).

**Diagnostic** - before treating "shows connected but isn't" as a Connected defect:

```bash
ss -tnpi | grep 1716    # look for Send-Q > 0, backoff:N, a large lastrcv
```

`1716` is KDE Connect's TCP port. Read the three fields together - any one alone is ambiguous:

- **`Send-Q` > 0** - bytes written to the socket that the peer has never acknowledged. On a healthy link this is 0 or drains within a second. A value that sits unchanged across successive `ss` runs is the strongest single signal that the phone is gone.
- **`backoff:N`** - the kernel is in exponential retransmission backoff, doubling the interval per attempt. Present only while data is unacknowledged, so it confirms what `Send-Q` suggests. `N` climbing between runs means the socket is working through its retransmission budget, not recovering.
- **`lastrcv`** - milliseconds since anything was received on the socket. Large and growing while `Send-Q` is non-zero is the half-open case. Large with `Send-Q` at 0 is just an idle link, which is normal.

The socket stays `ESTABLISHED` throughout, which is why the daemon keeps reporting the device as reachable and why there is nothing for Connected to key on.
