# ASR/Audit Support Desk - Staff User Guide

This guide uses simple words. Button names are shown exactly as they appear in the app.

## What You Can Do With This App

The app helps you manage ASR and Audit support emails from Outlook.

You can:

- find new support emails and turn them into tickets;
- see which staff member is working on a ticket;
- claim a ticket or assign it to someone else;
- keep notes, school information, and past actions in one place;
- use **Kanban**, **Team Overview**, and **Conflicts** to see team work;
- copy ticket information into Microsoft Copilot to help write a reply;
- open the Outlook reply, check it, and send it yourself;
- mark a ticket Done and keep its history;
- find related tickets and link or combine them when needed.

The app never sends a reply to a school. You review and send that reply in Outlook.

## Before You Start

Keep **classic Outlook** open while you use the app. The app connects to the Outlook program on your computer. It does not work with the new Outlook app.

Install the app with the supplied Windows installer. Then open **ASR Audit Support Desk** from the Start menu. Use the normal Windows account. Do not use **Run as administrator**.

The first time you open the app, set up Outlook, the folders to watch, the completion folder, and your staff information.

## First-time setup

### Connect to Outlook

1. Open **Settings**.
2. Click **Test Outlook Connection**.
3. Wait for the green success message.

If the test fails, open classic Outlook and try again.

### Choose the mailbox and folders

1. In **Monitored Folders**, click **Find Mailboxes**.
2. Choose the shared mailbox.
3. Choose **Inbox**, or another folder that your team uses.
4. Choose how folders inside that folder should be watched:
   - **Monitor this folder only** reads only the folder you selected.
   - **Include all child folders** reads that folder and every folder inside it.
   - **Include selected child folders** lets you choose only certain folders.
5. If you choose selected folders, tick **and its subfolders** when you want everything below that folder included.
6. Read the **Scanning:** message. It tells you exactly which folders the app will read.
7. Choose **All items in the folder (recommended)** unless your manager tells you to use **Unread items only**.

The app also finds child folders inside a shared mailbox when Outlook's normal folder list does not show them. If a folder exists but cannot be opened, the app reports the problem.

The completion folder is never scanned for new tickets. This prevents finished emails from becoming new tickets.

### Choose the completion folder

1. In **Completion Folder (Mark Done)**, click **Open**.
2. Choose the folder where finished emails belong, such as **Completed**.
3. Click **Validate**.
4. Continue only after the folder shows **OK**.

### Add staff members

1. In **Staff Assignment**, click **+ Add staff member** or **Edit** beside a name.
2. Enter the person's name and email address.
3. Click **Choose folder...** and choose that person's Outlook folder.
4. Click **Save staff member**.
5. Choose your own name beside **I am**.
6. If needed, use **Find accounts** under **Send notifications from**.
7. Click **Save Settings**.

A staff folder also includes folders inside it. Deactivating a person keeps their old history but stops new assignments to them.

### Test and save

1. Click **Test Outlook Connection** again.
2. Check the **Scanning:** message.
3. Check the completion folder shows **OK**.
4. Click **Save Settings**.

If a folder is missing, click **Find Mailboxes** again and check the parent folder. For a shared mailbox, try **Include all child folders** temporarily to see whether the folder appears. Then choose the smaller set of folders you actually need.

## Check for new emails

1. Click **Tickets**.
2. Click **Check Now**.
3. Read the result message.

The app reads the selected folders without marking emails read, moving them, or changing flags. It also checks where known emails are now. If a check is incomplete, try **Check Now** again later.

## Find a ticket

- **Current Tickets** shows tickets with an email in a folder you watch.
- **My Tickets** shows open tickets assigned to you.
- **Unassigned** shows tickets with no owner.
- **Ticket History** shows finished or moved tickets.

Use the search box for a school name, ticket number, sender, email address, phone number, or words from the email. Use **More filters** to filter by status, staff member, school, issue, or date. Click **Clear Filters** when a ticket seems to be missing.

## Work on a ticket

1. Open the ticket.
2. Check the school, sender, issue, and email details.
3. Add a school with **+ Add school** if needed.
4. Add an issue with **+ Add issue type...** if needed.
5. Write completed work under **Staff actions already taken**.
6. Write useful information for yourself or coworkers under **Staff notes**.
7. Click **Start Work** when you begin.

The app may show **Possible duplicate**, **Needs review**, or **Ownership conflict**. Read the message before continuing. Related tickets appear in **Related cases**. The app does not combine tickets automatically. You choose whether to link or combine them.

## Claim, Assign, and Reassign

| Action | Use it when | What happens |
|---|---|---|
| **Claim** | You will work on the ticket yourself | You become the owner. The email does not move and no handoff email is sent. |
| **Assign** | You give an unassigned ticket to a coworker | The app moves the incoming email to that person's Outlook folder and sends one handoff email to that person. |
| **Reassign** | Someone else is taking over | The app moves the incoming email to the new person's folder and keeps the assignment history. |

### Claim a ticket

1. Open the ticket.
2. Click **Claim**.
3. Click **Start Work** when you begin.

If another person owns it, the button is **Take over...**. The app keeps the earlier person's history.

### Assign or reassign a ticket

1. Open the ticket.
2. Click **Assign** or **Reassign**.
3. Choose the staff member.
4. Check the email address and Outlook folder shown in the dialog.
5. Add a handoff note if needed.
6. Click **Assign**.

The app moves only incoming school emails. It does not move replies, drafts, staff emails, or notifications. If a move fails, fix the problem and click **Retry assignment**. Already completed moves are not repeated.

The assignment box keeps the person's name, date, folder, note, and handoff result. If the handoff message failed, fix the problem and click **Retry Notification**. If the result is uncertain, click **Check Sent Items** before using **Send again...**.

## Write and send a reply

1. Click **Copy Prompt**.
2. Open Microsoft Copilot and paste the prompt.
3. Read Copilot's draft carefully. Check names, school IDs, dates, and completed actions.
4. Copy the reply from Copilot.
5. In the ticket, click **Open Outlook Reply** or **Reply All**.
6. Paste the reply into Outlook.
7. Check the recipients and text.
8. Click **Send** in Outlook.

The app does not connect to Copilot and does not send the message for you.

## Mark a ticket Done

1. Send the reply from Outlook.
2. Return to the ticket.
3. Click **Mark Done**.

The app records who finished the ticket and keeps its history. In standalone mode, the incoming emails are flagged complete and moved to the completion folder immediately. With shared ticket records, the app does this after the shared records confirm the Done decision. A later reply moves the ticket to **Review**.

If Mark Done reports a problem, read the message, fix the problem, and click **Retry Mark Done**. A Done ticket can be opened from **Ticket History**. Click **Reopen** to work on it again.

## Shared ticket records and working with others

The shared records file lets staff see the same tickets, owners, notes, statuses, and history. The default file is:

`\\epi-qb-5\VPD_Shared\PgmDocs\Assessment and Coverage\Annual Immunization Status Report\ASR Master List Revamp--2023\asr_email_support.accdb`

To check it:

1. Open **Settings**.
2. Open **Shared Database**.
3. Read the connection status.
4. Use **Test Connection** if needed. The same check is called **Test Database Connection** on **Diagnostics**.

To change its location, use the file location in **Settings > Shared Database**, then test and save it. Ask your manager before changing the shared file for the whole team.

If the shared file cannot be reached, the app may continue saving changes on your computer. The status shows that changes are saved locally and waiting to be shared. When the connection returns, click **Sync Now** or wait for the next check. Do not create a second shared file unless your manager tells you to.

Example: Aaron opens a ticket and starts work. Wenjie later takes over. The app keeps Aaron's earlier work and records Wenjie as the new owner.

If two people change the same ticket, open **Conflicts**. Read both values and choose **Keep shared value** or **Use my value**. The ticket's work status is not changed by reviewing a conflict.

## Team tools

- **Kanban** shows tickets in columns by status. You can filter the board and drag a card to another status. The same checks used by the ticket buttons still apply.
- **Team Overview** shows staff workload, older tickets, pending Outlook work, conflicts, and recent staff activity.
- **Conflicts** shows changes that need a decision.
- **Related cases** shows possible matches. Link cases when they belong together. Use **Combine cases** only after reviewing the preview.
- **Reports** shows totals and can create a CSV file.
- **SOP Templates** contains the approved wording used in Copilot prompts.

## Common Problems and How to Fix Them

| Problem | What to do |
|---|---|
| The app cannot connect to Outlook | Open classic Outlook. Close and reopen the app normally. Do not use Run as administrator. |
| A folder is missing | Check the parent folder, choose the correct folder mode in Settings, and click **Check Now**. |
| The shared file cannot be opened | Check the network connection and your permission. Use **Test Database Connection**. |
| A ticket is missing | Open **Ticket History**, clear filters, and click **Check Now**. |
| Assignment failed | Read the message, fix the Outlook folder or permission problem, and click **Retry assignment**. |
| The handoff email failed | Fix Outlook, then click **Retry Notification**. |
| Mark Done failed | Fix the message shown and click **Retry Mark Done**. |
| Two people changed the same ticket | Open **Conflicts** and choose the correct value. |
| Team Overview says a folder was not checked | Add that staff folder to the folders selected in Settings, then click **Check Now**. |
| Copy Prompt did not work | Select the text in **Copilot Prompt Preview**, press Ctrl+A, then Ctrl+C. |

### Send a problem report

1. Open **Diagnostics** from the left menu.
2. Click the event that shows the problem.
3. Click **Copy Log** or **Copy Selected**. You can also use **Export Log to File...**.
4. Paste the copied text into your message to the developer, or attach the exported file.

The log removes personal information where possible. Do not send a screenshot or file that contains information your team should not share.

## How to Test the App

Use made-up test emails and a test mailbox. Do not move, flag, or send real Outlook messages during testing.

| What to do | What should happen | Result |
|---|---|---|
| Open the app and test Outlook | A success message appears | Pass / Fail |
| Find a test email | A ticket appears after **Check Now** | Pass / Fail |
| Claim the ticket | You become the owner; the email does not move | Pass / Fail |
| Assign it to Aaron | The email moves to Aaron's folder and Aaron gets one handoff email | Pass / Fail |
| Open a second test instance | The second user can see the shared ticket and owner | Pass / Fail |
| Add a note | The note is saved and shared | Pass / Fail |
| Copy a prompt and review it in Copilot | The prompt contains the ticket information | Pass / Fail |
| Open the Outlook reply and send the test reply | Outlook opens the reply; staff sends it | Pass / Fail |
| Mark the ticket Done | The ticket is recorded in **Ticket History** | Pass / Fail |
| Reopen the ticket | The ticket can be worked on again | Pass / Fail |
| Disconnect the shared file | The app shows local saved changes or a pending change | Pass / Fail |
| Reconnect and click **Sync Now** | The pending change is shared | Pass / Fail |
| Open **Diagnostics** and copy a log | A log can be copied without programming commands | Pass / Fail |

Developer testing can use two app instances on one computer with separate local profiles. A later staff test still needs real staff accounts, test Outlook profiles, and a test network share. Never use live support emails for this test.
