# ASR/Audit Support Desk — User Guide

## The workflow at a glance

**Scan → Claim the case → Copy Prompt → paste into Copilot → Open Outlook Reply → paste Copilot's reply, review it and send it yourself → Mark Done**

One ticket is one **support case**. A case can hold several schools, several issue types and several emails, and one person is its current **owner** while others can add notes.

**Mark Done** records your decision immediately (no check of Sent Items is needed). If the sender writes again later, the case moves to **Review** so the reply is not missed.

With the shared team database, everyone sees the same cases, owners, notes and history. The app always saves on your computer first, so you can keep working when the network share is not available; your changes show **Saved locally — pending sync** until they reach the shared database.

The app never writes or sends the reply itself, and it does not connect to Copilot or any other AI service. You copy the prompt into Microsoft Copilot and paste Copilot's answer into Outlook yourself.

---

## 1. First-time setup (Settings)

1. **Outlook Connection.** Open classic Outlook, then click **Test Outlook Connection**. If it says the app and Outlook run at different permission levels, close the app and start it normally (not with *Run as administrator*).
2. **Monitored Folders.**
   1. Click **Find Mailboxes** and choose *DOH-Schoolimmforms-team, Vpdp [DOH]*. If it is not listed, type the mailbox name and click **Open**.
   2. In the folder tree, click the top folder to watch, usually **Inbox**.
   3. Choose which child folders are scanned too:
      - **Monitor this folder only**: Inbox itself.
      - **Include all child folders**: every subfolder, including *Completed* folders. That is usually too much in a shared mailbox.
      - **Include selected child folders** (recommended): tick your own folder, for example **Wenjie**, and then **and its subfolders** to include everything filed below it.
      
      The child folders appear automatically. The blue **Scanning:** line shows exactly what will be read, for example *Inbox (this folder only) + Inbox/Wenjie and all its subfolders*.
      
      The completion folder (step 3 below) is never scanned. Emails that colleagues later move to other subfolders, such as *Completed*, are still found by **Open Outlook Reply** and **Mark Done**, but they don't create new tickets.
   4. Choose **Scan**:
      - **All items** (recommended) imports every email in the folder.
      - **Unread items only** imports unread emails only. An email someone opened before the scan is skipped.
      - Scanning never marks emails as read, never moves them and never changes their flags.
3. **Completion Folder (Mark Done).** This setting is required before you can mark tickets Done.
   1. Click **Find Mailboxes**, or type a shared mailbox name and click **Open**.
   2. Click the folder where finished emails should go, for example *Inbox › Completed*. It may be in a shared mailbox. It cannot be a monitored folder.
   3. Click **Validate**. The folder is checked again when you save.
4. **Copilot Prompt.** These are the general instructions at the top of every prompt. You can change them; **Restore Default** brings back the original text.
5. **Schedule.** Turn on automatic checking and pick an interval (default 5 minutes). **First import** sets how many days back to read the first time.
6. **Ticket Preferences.** Enter your name, which is recorded in the ticket history. Add staff addresses or domains (for example `@doh.nj.gov`) so internal emails don't create tickets.
7. Click **Save Settings**.
8. **Staff Assignment.** For each person who can receive tickets, click **+ Add staff member**:
   - display name, the **email address** that receives handoff emails, and **their Outlook folder** (click **Choose folder…**, for example *Inbox › Aaron*). A folder also covers its subfolders unless another person has a more specific folder.
   - **Deactivate** keeps a person's name on existing tickets but blocks new assignments.
   - **I am**: your own name (used by **My Tickets**).
   - **Send notifications from**: an Outlook account you are allowed to send from (**Find accounts**). Then **Save Settings**.

Click **Check Now** (bottom left, or in the tray menu) at any time. Closing the window keeps the app running in the system tray.

---

## 1a. Team Tickets, My Tickets, Unassigned

These views show the **shared team cases**, whichever Outlook folder their emails are in:

- **Team Tickets** — every case. Filter by status, owner, issue, school, sender, dates, and **sync state** (pending, conflict, failed).
- **My Tickets** — open cases you own (choose who you are in Settings › Staff Assignment › **I am**).
- **Unassigned** — cases nobody owns yet.

The bar above the list shows the shared database state and when shared data was last refreshed. One computer's folder scan is not complete team coverage; that is what the views below are for.

**Statuses**

| Status | Meaning |
|---|---|
| **Unassigned** | Nobody owns the case |
| **Assigned to …** | Someone took responsibility (Claim); work has not started |
| **In Progress — …** | Work has started (**Start Work**) |
| **Review — …** | A Done case received a new reply, or someone flagged it for review (the reason is shown). The owner is kept |
| **Done — …** | Someone clicked **Mark Done** |

Sync problems and pending Outlook work are **badges**, not statuses: *pending sync*, *conflict*, *sync failed*, *new reply*.

## 1b. Current Tickets, Awaiting Location Check, Ticket History

- **Current Tickets** shows a ticket only when at least one of its emails is **now** in the selected folders (the completion folder never counts). A ticket with emails in several folders is shown once; each email shows whether it is in the current folders.
- **Awaiting Location Check** (formerly *Awaiting Verification*) appears after an update or after you change the folder selection: the app has not yet confirmed where these tickets' emails are. Click **Check Now**. They are not claimed to be current. (This is about email location only; replies never need to be "verified" before Mark Done.)
- **Changing the folder selection** immediately stops showing the previous folder's results as current; nothing is deleted — the cases stay in Team Tickets and History.
- **Ticket History** holds earlier tickets whose emails are no longer in the selected folders, with the reason: *Completed*, *Reassigned outside the selected scope*, *Moved elsewhere*, *No longer found after successful reconciliation* (never "deleted"). Opening one changes nothing; you can still copy the prompt and open the email if Outlook can find it.
- Every check first imports new emails, then verifies where all known emails are now — independent of the date window and the unread-only setting. A check that fails or is incomplete never moves tickets to History. Emails moved by hand are followed without duplicates; an email that comes back into your folders brings its ticket back.
- The grey bar above the list shows the mailbox, the selected folders, the last successful verification, and whether a check is running, incomplete or stale.

---

## 2. Finding tickets

The left panel lists tickets:

- **Search** looks through:
  - ticket number (`T-000012`)
  - school names, NJIIS IDs (including leading zeros) and other school IDs
  - addresses
  - sender name, title, email and phone
  - subjects and email text
  - issue types
  - staff notes and staff actions

  It works without Outlook running.
- **Status:** All, **In Progress** (the default) or **Done**.
- **Issue type:** matches any of a ticket's issue types.
- **Received** dates filter on the date the email arrived. **Completed** dates filter on the date the ticket was marked Done. Both end dates are included, in your local time.
- **Sort by** received date, last updated or completed date.
- **Clear Filters** resets everything. The number of matching tickets is shown below the filters.

Selecting any ticket, including an old Done ticket, opens it in full: the emails, fields, prompt and history.

---

## 3. Working a ticket

The ticket workspace has the action bar at the top, which stays visible while you scroll.

| Button | What it does |
|---|---|
| **1. Copy Prompt** | Builds the Copilot prompt from the saved ticket and copies it to the clipboard. You see "Prompt copied". Works without Outlook. |
| **2. Open Outlook Reply** | Opens Outlook's own Reply window for the original email, in the same conversation, with your signature and the quoted email. Nothing is typed into it and nothing is sent. |
| **Reply All** | The same, using Reply All. |
| **Copy Prompt & Open Outlook Reply** | Copies first, then opens Outlook. Each step reports its own result, so a clipboard problem and an Outlook problem are easy to tell apart. |
| **3. Mark Done** | Flags the ticket's incoming email(s) complete in Outlook, moves them to the completion folder, then sets the ticket to Done. |

Copying the prompt, opening a reply, or saving changes **never** marks the ticket Done.

### Typical steps

1. **Review the ticket.**
   - Check the **Issue** chips. Add more issues with **+ Add issue type…** or remove one with ✕. The first chip is the primary issue.
   - Check the **Sender** and **School** fields. Correct anything that is wrong; changes are saved when you leave a field.
   - If one email covers several schools, each school has its own group, so IDs and addresses stay with the right school. Use **+ Add school** or **Remove**.
   - Record what you already did under **Staff actions already taken**, for example "Reset the account on 11/17". The prompt tells Copilot to describe an action as completed **only** if it is recorded there.
2. **Copy Prompt.** The **Copilot Prompt Preview** shows exactly what goes on the clipboard.
3. **Paste the prompt into Microsoft Copilot** and let it draft the reply.
4. **Open Outlook Reply**, paste Copilot's reply into the Outlook window, check it, and click **Send** in Outlook.
5. **Mark Done.**

### Missing information

Empty fields show **[Not provided]** in the prompt. You can still copy the prompt and open a reply; the prompt asks Copilot to request essential missing details in the reply.

### Field labels

- *Extracted – Not Verified*: the value was found in the email.
- *Entered*: you typed the value.
- *Verified*: you checked the value (**✓ verify**).
- **use email value** brings back what the email said.

Later scans never overwrite your corrections.

### Badges

These are hints only; tickets have just two statuses.

- **Check issue type**: the issue type was guessed with low confidence.
- **Possible duplicate**: another ticket has the same sender or school and the same issue. Tickets are never merged automatically. Click **Not a duplicate** to clear it.
- **Needs review**: flagged for a second look. Click **Reviewed — clear flag** to clear it.

### If the original email can't be found

If the original email was deleted or moved somewhere the app can't see, **Open Outlook Reply** says so. It then offers **Open new message to sender**. That message is **not** threaded with the original conversation, and it is only opened if you click the button.

---

## 4. Claim, Start Work, Review, Mark Done

**Claim** makes you the owner. Nothing moves in Outlook and nobody is emailed. If someone else already owns the case, the button is **Take over…**: their notes, history and ownership dates are kept. **Clear owner** makes the case Unassigned again.

If two people claim the same case at the same time, the shared database accepts the first one. The other person sees the confirmed owner and a **Conflict** panel ("Aaron claimed this case first") with **Keep shared value** or **Take over…** — never a silent change.

**Start Work** (owner required) → In Progress. **Flag for Review…** asks for a reason; **Review handled** continues the work.

**Mark Done** records your decision immediately — also offline. It saves who, when and which incoming emails it covers. No sent-reply check, and no email is sent to anyone.

- *Standalone computer:* the app then flags the covered emails complete in Outlook and moves them to the completion folder. If that fails, the case **stays Done** and shows the Outlook problem; **Archive in Outlook** retries only the missing steps.
- *Shared database:* Outlook is not changed right away. Once the shared database confirms the Done decision, use **Archive in Outlook** to flag and move the covered emails. A reply that arrived later is never archived under the older decision.

**New replies.** If the sender writes again after Done, the case moves to **Review — New reply from …** for everyone, with the owner kept. If the reply arrived *while* you were marking Done (for example you were offline), your Done is not applied; you see a conflict explaining why and **Mark Done again** after you handled the reply. Older emails found later, automatic replies (out of office, undeliverable) and staff emails do not reopen a case.

**Reopen** puts a Done case back to In Progress (or Unassigned). The archived email stays where it is.

**Add to existing case…** puts this case into another case (enter its T-number or ASR reference): its emails, notes and issue types move there and the old reference keeps working. Nothing changes in Outlook. Cases are never combined automatically just because the sender or school is the same.

**Notes** are separate entries with author and time; notes from different people never overwrite each other.

---

## 5. SOP Templates

The approved SOP wording for each issue is still kept in **SOP Templates**, and the Copilot prompt includes the matching text for the ticket's issues. Edits you make there are used in the prompt.

- **Default Templates** lists every template by issue type, with its SOP section and rules. *edited* marks a template you customized.
- **Restore System Default** brings back the SOP text. Every save is kept in **Version history**.
- Old saved drafts and template history are kept, but the built-in reply editor is no longer part of the ticket screen.

---

## 6. Reports and backup

- **Reports** shows ticket counts by status, issue type and week. **Export CSV** opens in Excel; it contains ticket columns and field values, but not email text.
- **Settings › Database / Backup:**
  - **Back Up Database…** writes a consistent copy.
  - **Restore From Backup…** checks the file and saves a safety copy of the current data first.
- When the app is updated and the database format changes, a backup is written automatically to `%APPDATA%\gov.nj.doh.asrsupportdesk\backups\pre-migration-v<N>-<date>.db` before the upgrade.

---

## 6a. Assigning tickets (Outlook moves and handoff)

> **With the shared team database**, use **Claim / Take over** for now. Assign/Reassign (which moves emails into a staff folder and sends a handoff email) returns in the next release, when moves are reserved in the shared database first so two computers can never move or notify twice. On a **Standalone** computer, Assign works as described below.

1. Open a ticket and click **Assign** / **Reassign**, or tick several tickets in the list and click **Assign selected**.
2. Choose the person. The dialog shows their notification email, destination folder, number of tickets and emails, and whether a notification will be sent. Add an optional handoff note and click **Assign**.
3. The app moves the ticket's emails from the school into the person's folder (not your replies or drafts; nothing is flagged or marked read). **Only after all moves succeeded** is the ticket set to *Assigned* and the handoff email sent — to that staff member only, never to the school.
4. Each ticket reports its own result.

Problems:

- **Not finished** (a move failed): fix the reason, then **Retry assignment**. Emails already moved are not moved again; no email is sent until everything moved.
- **Assigned — notification failed**: the assignment stands. Fix the reason and click **Retry Notification** (emails are not moved again).
- **Notification outcome uncertain**: Outlook may have sent it. Click **Check Sent Items**; only if it is not there, use **Send again…**. The app never resends on its own.
- *Submitted to Outlook* means Outlook accepted the email; delivery is not confirmed.
- Reassigning to the **same person** sends no new email. A **Done** ticket must be **Reopened** before it can be assigned.
- If emails of one ticket end up in two people's folders, the ticket shows **Ownership conflict** and the owner is not changed automatically.

**Team Overview** (left menu) counts tickets per person. Unassigned / Assigned / In Progress come from emails confirmed in your selected folders; Done counts are this computer's records for the chosen dates. Staff folders outside your selected folders are marked *not reconciled*.

**Several computers:** with the shared database, cases, owners, statuses, notes and history are shared (see *Settings › Shared Database* and [SHARED_DATABASE.md](SHARED_DATABASE.md)). Moving an email into someone's folder by hand never changes the owner by itself once the shared database is connected; it is shown on the email. T-numbers are local to each computer; the *ASR-…* reference is the same everywhere.

---

## 6b. Diagnostics (when something does not work)

Left menu › **Diagnostics** (also **Open Diagnostics** in Settings › Shared Database).

- The top shows the app version, the shared database state, the last successful sync and how many changes are waiting.
- The list shows recent technical events: start-up, shared database checks (with codes such as `SHARE_UNAVAILABLE`, `DB_FILE_NOT_FOUND`, `DB_PERMISSION_DENIED`, `DB_READ_ONLY`, `DB_LOCKED`, `ACCESS_DRIVER_MISSING`, `ACCESS_DRIVER_ARCH_MISMATCH`, `DB_MAINTENANCE`, `DB_IDENTITY_CHANGED`), synchronization results, Outlook checks and Outlook actions. Click an event to see its details.
- **Errors only / Warnings and errors** filters the list; **Go to latest error** jumps to the newest error.
- **Copy Log** copies the filtered list as plain text; tick events and use **Copy Selected** for just those. Paste it into an email or Teams message to the developer. If the clipboard is blocked, the text is shown so you can copy it by hand.
- Email addresses, phone numbers, school and NJIIS IDs, subjects, Windows user names and connection strings are removed before anything is saved or copied. Email text and Copilot prompts are never logged. Diagnostics are kept on this computer (the most recent few thousand events) and are separate from the case history.
- A problem with Diagnostics itself never stops you from saving or synchronizing.

---

## 7. What the app does and does not do in Outlook

| Action | When |
|---|---|
| Read emails (never marks them read) | Scans and verification (Check Now and scheduled checks) |
| Open a Reply / Reply All / new-message window | Only when you click the button. You send the email yourself |
| Move emails to a staff member's folder | **Only** when you click **Assign** / **Reassign** (or Retry assignment) |
| Send a handoff email to the assigned staff member | **Only** after a successful assignment you started (or Retry Notification) |
| Flag emails complete and move them to the completion folder | Standalone: right after **Mark Done**. Shared database: **only** when you click **Archive in Outlook** after the Done decision is confirmed. Only emails covered by that decision |
| Change Outlook because of **Claim**, **Take over**, **Review**, notes or **Add to existing case** | **Never** |
| Send to schools/senders, delete, mark read/unread | **Never** |

The app also never stores your Outlook password, never sends data to external or AI services, and never overwrites values you entered or verified.
