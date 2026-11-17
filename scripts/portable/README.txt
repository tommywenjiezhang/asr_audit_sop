ASR/Audit Support Desk - portable version {{VERSION}} (Windows 10/11, 64-bit)
=========================================================================

This ZIP contains everything the app needs. You do NOT need Python, Node.js,
Rust or administrator rights.


1. Extract first
----------------
- If you downloaded the ZIP from email/Teams/SharePoint: right-click the ZIP,
  choose Properties, tick "Unblock" (if shown), click OK.
- Right-click the ZIP -> "Extract All..." and extract the WHOLE ZIP to a folder
  you own, for example  C:\Users\<you>\Apps\ASR Audit Support Desk
  (spaces in the folder name are fine).
- Do NOT start the app from inside the ZIP window. It needs the
  "outlook_worker" folder next to it.


2. Start the app
----------------
Double-click:   ASR Audit Support Desk.exe

- Start it normally. Do NOT use "Run as administrator" - Windows then blocks
  the connection to Outlook.
- If Windows shows "Windows protected your PC", the app is not code-signed.
  Ask your IT team before choosing "More info" -> "Run anyway".
- Only one copy runs at a time. If the installed version is open, close it
  first (also from the system tray near the clock).
- The illustrated "ASR-Support-Desk-User-Guide.html" explains every step.


3. Requirements on the computer
-------------------------------
- Classic Outlook for Windows (not "new Outlook"), signed in, with access to
  the help-desk mailbox (for example the shared mailbox) in your profile.
  Keep Outlook open while you use the app.
- Microsoft Edge WebView2 Runtime. It is part of Windows 11 and is installed
  on most Windows 10 PCs with Microsoft Edge.
  If the app window stays blank or does not open:
    Settings -> Apps -> Installed apps -> look for
    "Microsoft Edge WebView2 Runtime". If it is missing, ask IT to install it
    (Microsoft's "Evergreen" WebView2 Runtime:
     https://developer.microsoft.com/microsoft-edge/webview2/ ).
- Mark Done needs permission to move emails into the completion folder you
  choose in Settings.


4. Where your data is stored
----------------------------
Your tickets, settings and logs are NOT stored in this folder. They are in
your Windows profile:

    %APPDATA%\gov.nj.doh.asrsupportdesk\
        support_desk.db    tickets, emails, history
        settings.json      your settings
        logs\              daily logs (no email text)
        backups\           automatic safety copies

(Paste %APPDATA%\gov.nj.doh.asrsupportdesk into the File Explorer address bar
to open it.) The installed version of the app uses the same folder, so you can
switch between the installed and the portable version.


5. Updating
-----------
1. Close the app (also from the system tray).
2. Extract the new ZIP into a NEW folder (or delete the old app folder first).
3. Start the new "ASR Audit Support Desk.exe".
Your tickets and settings stay in %APPDATA% and are kept. When a new version
upgrades the database it first writes a backup to the "backups" folder.
If you use "Start minimized ... when I sign in": in Settings turn it off and
Save, then turn it on and Save again, so Windows starts the new copy.


6. Removing
-----------
Delete the app folder. To also remove your data, delete
%APPDATA%\gov.nj.doh.asrsupportdesk (this deletes all tickets).


What the app does in Outlook
----------------------------
- Checking for new email only READS (nothing is marked read, moved or flagged).
- "Open Outlook Reply" only opens a reply window. YOU send replies to schools.
- "Assign" moves the ticket's emails into the staff member's folder and then
  sends ONE handoff email to that staff member (never to the school).
- "Mark Done" flags the ticket's emails complete and moves them to your
  completion folder. Nothing is ever deleted.
- Each computer keeps its own database: notes, statuses and history are not
  shared between computers. Outlook folders (who owns a ticket) are shared.

Build {{BUILD}} - files and SHA-256 hashes are listed in MANIFEST.txt.
