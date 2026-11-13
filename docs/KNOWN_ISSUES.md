# Known Issues

Known issues and workarounds in Connected.

## Encrypted RCS Conversations Missing Messages on Android 17

On Android 17, messages in end-to-end encrypted RCS conversations do not reach the desktop. This covers messages you receive and messages you send from the phone. Other conversations sync normally. Nothing reports an error; the messages are simply missing. This happens on the phone, not in the applet.

### Symptoms
- A message is visible in the phone's messaging app but never appears in Connected
- The affected conversations are end-to-end encrypted (Google Messages: open the conversation's details)
- SMS, MMS and unencrypted RCS conversations are unaffected
- Started after the phone was updated to Android 17

### Technical details

Stock Android 17 lets the messaging app mark end-to-end encrypted messages as restricted in the phone's message store, and Google Messages does so. Apps without restricted-message access cannot read them. KDE Connect does not have that access by default, and it cannot request it either, so the messages never leave the phone. This is part of Android itself, not a manufacturer change, so it applies to any phone on Android 17. Every KDE Connect desktop client is affected, not only Connected.

### Workaround

Grant KDE Connect restricted-message access over adb (USB debugging enabled, phone connected):

    adb shell appops set org.kde.kdeconnect_tp READ_RESTRICTED_MESSAGES allow

Then restart the desktop side so the daemon fetches the missing messages again:

    killall kdeconnectd
    killall cosmic-panel

If the phone does not reconnect, open KDE Connect on the phone and refresh the device list. An Android or KDE Connect update may reset the grant; check it with:

    adb shell appops get org.kde.kdeconnect_tp READ_RESTRICTED_MESSAGES

To undo, set it back to `default`.

## Group Messages and Attachments Sent from the Desktop Never Confirm on Android 17

On Android 17, an MMS sent from the desktop (any group message, or any message with an attachment) is delivered to its recipients but is never recorded as sent on the phone. Connected therefore never receives the sent copy: the message does not appear in the conversation, and after a minute the reply box reads "Not confirmed by your phone". One-to-one text messages go as SMS and are unaffected. This happens on the phone, not in the applet.

### Symptoms
- The recipients receive the message
- The message does not appear in the conversation, and the note under the reply box changes from "Sending..." to "Not confirmed by your phone" after a minute. The note is gone once you leave the conversation.
- The phone's messaging app shows the send as "Not sent", often twice, or does not show it at all
- One-to-one text messages from the desktop confirm normally

### Technical details

Stock Android 17 changed how an MMS from an app other than the default messaging app is sent: it first offers the message to the default app, and when that app declines, sends it anyway and saves it a second time over the first copy. The send succeeds, but the stored message then disappears from the phone, so KDE Connect has nothing to read back and never reports it to the desktop. This is part of Android itself, not a manufacturer change, so it applies to any phone on Android 17. What removes the stored message is not yet known. The same happens with the stock KDE Connect SMS app, so it affects every KDE Connect desktop client, not only Connected.

### Workaround

Send group messages and attachments from the phone until this is resolved. Before resending a message that shows "Not sent", check with a recipient whether it already arrived.

## Non-Media Attachments Are Invisible in Threads

An MMS whose attachment is a PDF, document or other non-media file renders as an empty bubble. The file itself is sent and delivered correctly. This is a KDE Connect Android limitation, not an applet bug.

### Symptoms
- Sending a PDF with no accompanying text produces an empty bubble carrying only a timestamp, once the phone confirms the send
- Received non-media attachments show the same empty bubble
- The conversation list preview for the thread is also empty, since there is no body and no attachment flag
- Images, videos and audio in the same thread render normally

### Technical details

This limitation exists in the KDE Connect Android app which only supports audio, text, image and video images. This affects every KDE Connect desktop client, not only Connected

## Attachment-Only MMS May Render Without Its Attachment

An attachment sent with no text delivers to the recipient correctly and usually shows in its own bubble in Connected. Occasionally the sent bubble carries only a timestamp, with no pattern found. When it happens the daemon has reported the message without attachment metadata, which appears to be a KDE Connect limitation, not an applet bug.

### Symptoms
- The attachment arrives on the recipient's phone either way
- Occasionally the sent bubble shows a timestamp and nothing else, with no image and no placeholder
- The same attachment sent with text has not been seen to fail

### Technical details

In the captured case the daemon emitted `eventField 0`, an empty body and an empty attachment list, while the same image sent with text arrived as `eventField 1` with a populated list. Both came in one bulk `conversationUpdated` batch, a full re-fetch from the phone's message store, so the metadata was missing on re-read, not only in the live echo. Connected skips the empty body and the empty list, leaving the timestamp caption.

Captured 2026-07-26 against kdeconnect 23.08.5 on a self-thread. Most sends on the same daemon render normally.

## Text Sent with a Photo Is Missing

When a photo is sent or received with text, Connected may show the photo but not the text. The phone stores the photo and its text as two messages with the same timestamp, and KDE Connect's desktop daemon keeps only one message per timestamp in a conversation. The text is dropped before it reaches any desktop client. This is a KDE Connect bug, not an applet bug. It is fixed in KDE Connect 26.12 ([KDE bug 528719](https://bugs.kde.org/show_bug.cgi?id=528719)).

Captured 2026-10-18 against kdeconnectd 23.08.5 with Google Messages on a Galaxy S24.

Captured 2026-10-14 against kdeconnect 23.08.5 with Google Messages on a Galaxy S24+.

## Popup Does Not Reopen After a Double-Click in the File Chooser

Choosing an attachment opens the system file chooser, which takes focus and closes the applet popup. Connected reopens the popup when the chooser returns, chosen or cancelled, except when the file was chosen by double-click.

### Symptoms
- Double-click a file: the dialog closes and the popup stays closed
- Open, Enter or Escape: the popup reopens
- Nothing is lost: the file is staged and shows as a chip once the popup is open

### Technical details

The chooser activates on the second press, so the reopen is requested while the mouse button is still held and the popup never maps. The exact refusal has not been captured.

### Workaround

Pick with Open or Enter, or click the panel icon once after a double-click.

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
