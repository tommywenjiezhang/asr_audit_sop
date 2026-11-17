# ASR/Audit Support Desk — User Guide

## 1. First-time setup (Settings)

1. **Outlook Connection.** Open classic Outlook, then click **Test Outlook Connection**. If it says the app and Outlook run at different permission levels, close the app and start it normally (not with *Run as administrator*).
2. **Monitored Folders.**
   1. Click **Find Mailboxes** and choose *DOH-Schoolimmforms-team, Vpdp [DOH]*. If it is not listed, type the mailbox name and click **Open**.
   2. In the folder tree, click **Inbox › Wenjie** (or your own folder).
   3. Choose whether to include child folders: *School Subtype Needed*, *Verify Receipt of ASR/Audit*, or all of them.
3. **Schedule.** Turn on **Check the monitored folder automatically** and pick an interval (default 5 minutes).
   - **First import** sets how many days back to read the first time.
   - Optional: **Start minimized … when I sign in**.
4. **Ticket Preferences.**
   - Enter your name.
   - Optional: add staff addresses or domains (for example `@doh.nj.gov`) so internal emails don't create tickets.
5. Click **Save Settings**.

Click **Check Now** (bottom left, or in the tray menu) at any time. Closing the window keeps the app running in the system tray.

## 2. Working a ticket

The ticket screen has three panels. Drag the borders to resize them.

**Left — queue.** Filter by status in the side menu (New, Needs Review, In Progress, Waiting, Resolved/Closed) or by issue type. Search by school, NJIIS ID, sender, subject or ticket number (`T-000012`).

**Center — ticket details.**

- **Status / Priority / Issue Category.** The category is detected automatically. Confirm or change it, and the form changes to show the fields for that issue.
- **Original Email.** Shows the email text and any replies in the same conversation.
- **School & Contact** and **issue fields.** Values found in the email are labelled *Extracted – Not Verified*.
  - Type to correct a value; it is saved when you leave the field.
  - Click **✓ verify** after you have checked a value in NJIIS.
  - **use email value** brings back what the email said.
  - Empty fields show *Not Provided*. You never have to fill everything in.
- **School status** reads **Unverified School** until a School NJIIS ID is known.
- A **Possible duplicate** banner means another open ticket has the same sender or school and the same issue. Tickets are never merged automatically.

**Right — Email Assistant.**

| Button | What it does |
|---|---|
| Template list | Pick any variant for this issue (for example *Request subtype* or *Confirm subtype updated*) |
| **Load Default** | The original SOP text for the selected variant, placeholders included |
| **Generate Email** | Picks the right variant for the ticket's current state and fills in what is known |
| **Fill from Ticket** | Fills remaining `{{placeholders}}` after you add information. Your own edits are kept |
| **Reset to Default** | Brings back the original system text. Ticket data is not touched |
| **Save Draft** | Saves locally. Every save keeps the previous version (**Version history**) |
| **Create Outlook Draft** | Creates a reply in your Outlook Drafts folder, in the original conversation, with the SOP screenshots inline. **Nothing is sent.** The ticket status does not change |

**Yellow placeholders** mark missing information. Fill the field in the ticket, or type over the placeholder in the email.

A **red "Not verified" warning** means the selected text confirms something the ticket has not verified, such as "I can confirm we received the data…". Mark the field as verified, or choose a different template.

If placeholders or unverified statements remain when you create an Outlook draft, the app asks you to confirm. The draft subject then starts with **[INCOMPLETE DRAFT]**, and remaining placeholders are highlighted so you can finish the email in Outlook.

### Examples

- **Missing subtype.**
  - Subtype unknown: the app suggests *Request subtype*.
  - After you enter the subtype: *Subtype received – update pending*.
  - After **Subtype Updated in NJIIS = Yes** (verified): *Confirm subtype updated*.
- **Confirm receipt.**
  - Follow the staff steps shown under *Staff verification steps*.
  - Then set **Submission Verification Status**.
    - *Verified Submitted*: tick the grades you found. The confirmation lists them.
    - *Verified Not Submitted*: the app uses the not-submitted reply.
  - Until you verify, only the *Verification pending* reply is generated.
- **New user.** Enter the details you already have. The request email asks only for what is still missing.
- **LHD access denied.**
  - If the facility selection is unknown, the reply explains how to change the Current Facility.
  - If **Correct Current Facility Selected = Yes** and **Access Still Denied = Yes**, the reply asks for troubleshooting details instead.

Use **+ New** to create a ticket for a phone call or for an email outside the monitored folder.

## 3. Email Templates

- **Default Templates** lists every template by issue type, with its SOP section, rules and required fields.
  - *Review Required* marks a starter where the SOP has no complete email.
  - *edited* marks a template you have customized.
- Edit the text with the toolbar. **Insert field…** adds a placeholder. **HTML** shows the source.
- **Preview: sample data** shows the template with example values. **Preview: ticket** shows it with a real ticket's data.
- **Duplicate** creates a copy that you select manually. **Disable** hides a template from selection.
- **Restore System Default** brings back the SOP text. Every save is kept in **Version history**.
- **Issue Types** shows all categories, their SOP references and fields.

## 4. Reports

The Reports page shows ticket counts by status, issue category and week, plus follow-ups that are due.

**Export CSV** opens in Excel. It contains all ticket columns and field values, but not email text.

## 5. Backup

Use **Settings › Database / Backup**:

- **Back Up Database…** writes a consistent copy of the database.
- **Restore From Backup…** checks the file first and saves the current data to a safety copy before replacing it.

## 6. What the app never does

- It never sends, deletes, moves, archives or marks emails as read.
- It never stores your Outlook password.
- It never sends data to external or AI services.
- It never overwrites values you entered or verified, or email text you edited.

Remember the SOP's own process steps still apply: close the loop, mark emails complete, and move them to *Completed* in Outlook yourself.
