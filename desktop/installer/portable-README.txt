Let's Chat - Windows portable build
===================================

This is the no-install ("portable") build of the Let's Chat desktop app. It is
the same program as the NSIS installer (lets-chat-desktop-setup-windows-x86_64.exe),
just delivered as a plain folder you can run from anywhere - a USB stick, your
Downloads folder, wherever. Nothing is written to Program Files and no Add/Remove
Programs entry is created; to "uninstall", delete the folder.

What's in this zip
------------------

  lets-chat-desktop.exe            The app. Double-click to run.
  MicrosoftEdgeWebview2Setup.exe   Microsoft's WebView2 runtime installer.
                                   Only needed if the app shows a blank window
                                   (see "WebView2 runtime" below).
  README.txt                       This file.


First run: SmartScreen / "Windows protected your PC"
----------------------------------------------------

These binaries are not code-signed (we have no signing certificate yet), so the
first time you launch lets-chat-desktop.exe Windows SmartScreen may show a blue
"Windows protected your PC" dialog naming an "Unknown publisher". This is
expected for unsigned software and does not mean the file is unsafe.

To run it anyway:

  1. In the SmartScreen dialog, click "More info".
  2. Click the "Run anyway" button that appears.

Alternatively, before launching: right-click lets-chat-desktop.exe ->
Properties -> on the General tab, tick "Unblock" near the bottom -> OK. Then
double-click as normal.


WebView2 runtime (only if the window is blank)
----------------------------------------------

The app renders its UI inside the Microsoft Edge WebView2 runtime. Windows 11
and most up-to-date Windows 10 machines already have it. If the app opens to a
blank or white window, the runtime is missing: run
MicrosoftEdgeWebview2Setup.exe once (it downloads and installs the runtime
online, a few seconds), then re-launch lets-chat-desktop.exe.


Updating
--------

The desktop app updates itself in place, so you do not need to re-download this
zip for new versions - just keep using the same lets-chat-desktop.exe.
