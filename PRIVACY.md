# Privacy: your sign-in cookie

To show your playlists, this app needs the same "cookie" your browser uses for
YouTube Music. A cookie is like a temporary password: whoever has it can use
your account. This page says what the app does with it and how you can check
that yourself, without having to trust us.

## What the app does with it

- **Checks it once.** When the sign-in window hands it over, the app makes
  one request to YouTube Music. If YouTube rejects it, nothing is saved.
- **Encrypts it.** It is stored encrypted with your Windows account (Windows
  DPAPI). Another Windows user, or someone who copies the file to another PC,
  cannot read it.
- **Keeps it on this PC.** The encrypted file lives in
  `session.bin` in the app's data folder (by default `%LOCALAPPDATA%\zephyr`,
  changeable in Settings > Data folder). It is never uploaded, synced or
  backed up by the app.
- **Sends it only to YouTube.** The cookie goes only to
  `music.youtube.com` and `www.youtube.com`. This is enforced in one place in
  the code, and a test fails if anyone changes that list. Requests that carry
  it never follow redirects, so they cannot be bounced to another server.
- **Never shows or logs it.** The app writes no log files and sends no
  telemetry.
- **Forgets it on sign out.** Settings > Account > Sign out deletes the
  encrypted file and the sign-in window's profile, and clears your lists from
  memory.

## If you use "Sign in with Google"

- **You type your password into Google, not into the app.** The window is
  Microsoft's WebView2 (the engine inside Edge) showing Google's real sign-in
  page. The app adds no scripts to that page and does not read what you type.
- **The app only takes the cookie at the end.** Once the window reaches
  music.youtube.com, the app reads the music.youtube.com cookie from WebView2,
  closes the window, then checks, encrypts and stores it as above.
- **The window keeps its own login.** Like a browser, WebView2 remembers that
  you are signed in to Google, in the `webview` folder inside the data
  folder. WebView2 stores it the way Edge stores its own cookies. Sign out
  deletes this folder too.
- **Code:** `src/web_sign_in.rs`.

## Every server the app talks to

| Server | What for | Gets your cookie? |
|---|---|---|
| `music.youtube.com` | Your library, Home, Explore, recommendations | Yes |
| `www.youtube.com` | View counts; finding the audio stream | Only for view counts |
| `*.googlevideo.com` | The audio itself | No |
| `i.ytimg.com`, `*.googleusercontent.com`, `*.ggpht.com` | Cover images | No |
| `api.github.com` | Checking for a new Zephyr release, once per start and when you press Check again | No |
| `accounts.google.com` and the servers Google's sign-in page uses | Only while the sign-in window is open | That window has its own Google login |

If YouTube's direct audio lookup fails, the app asks the separate program
`yt-dlp` to find the stream. yt-dlp never receives your cookie.

## Check it yourself

You do not have to take our word for any of this.

**Watch the connections while the app runs**

1. Press `Win + R`, type `resmon` and press Enter (Windows Resource Monitor).
2. Open the **Network** tab and tick `Zephyr.exe` in the list at the top.
3. Under **TCP Connections**, every remote address the app is talking to is
   listed. All of them should belong to Google (YouTube and its image and
   video servers), apart from GitHub for the update check.
4. To see names instead of numbers, open PowerShell and run
   `Resolve-DnsName <address>`, or use Microsoft's free **TCPView** tool.

**Read the code that touches the cookie** (it is short)

- `src/api.rs`: `COOKIE_ORIGINS` lists the only two servers that may receive
  it; `post_to` is the only place that attaches it.
- `src/credentials.rs` and `src/dpapi.rs`: how it is encrypted, saved and
  deleted.
- `src/auth.rs`: how it is checked, and wiped from memory.

## Cut off access at any time

If you ever change your mind, signing out in the app deletes the local copy.
To make the cookie useless everywhere, also go to your Google Account >
**Security** > **Your devices** and sign out of the session, or change your
password. That invalidates the cookie on Google's side within moments.

## What we cannot promise

- The app wipes the cookie from its own memory when it is done with it, but
  some short-lived copies made while handing it over from the sign-in window
  are freed without being wiped. They live only in this app's memory and
  disappear when it closes.
- Any program that runs as your Windows user could, in principle, read the
  encrypted file and decrypt it, just as it could read your browser's cookies.
  Keep your PC free of malware.
- The release `.exe` is built from this source code, but you cannot yet
  verify that byte for byte. Reproducible builds with signed build records are
  planned, so anyone can confirm the download matches the public source.
