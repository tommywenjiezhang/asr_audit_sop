# How to Test the App

This is a short staff test. Use a test mailbox, test folders, and made-up email messages. Do not move, flag, or send real Outlook messages during testing.

## Before testing

1. Install the current release with a normal Windows account.
2. Open classic Outlook.
3. Open the app and complete **Settings**.
4. Choose a test mailbox, test folders, and a test completion folder.
5. Add two test staff members with test email addresses and test Outlook folders.
6. Click **Save Settings**.

## Test steps

| What to do | What should happen | Result |
|---|---|---|
| Test Outlook | A success message appears | Pass / Fail |
| Click **Check Now** | A test email becomes a ticket | Pass / Fail |
| Click **Claim** | You become the owner; Outlook does not change | Pass / Fail |
| Assign the ticket to Aaron | The email moves to Aaron's test folder and one handoff email is sent | Pass / Fail |
| Open a second app instance | The second user sees the ticket and its owner | Pass / Fail |
| Add a note | The note is saved and appears for the other user | Pass / Fail |
| Use **Kanban** | The ticket appears in the correct status column | Pass / Fail |
| Open **Team Overview** | Workload and pending work are shown | Pass / Fail |
| Change the same field in both instances | A conflict appears in **Conflicts** | Pass / Fail |
| Choose **Keep shared value** | The conflict closes with the shared value | Pass / Fail |
| Mark the ticket Done | The ticket appears in **Ticket History** | Pass / Fail |
| Reopen the ticket | The ticket can be worked on again | Pass / Fail |
| Make the shared file unavailable | The app reports the problem and keeps local changes pending | Pass / Fail |
| Restore the shared file and click **Sync Now** | The pending change is shared | Pass / Fail |
| Open **Diagnostics** and click **Copy Log** | A log can be copied for the developer | Pass / Fail |

## Two instances on one computer

For developer testing, use `scripts\launch_multiuser_test.ps1` to start two instances with separate local profiles. Point both instances to the same test shared file. Do not point the test instances at the production shared file.

## If a test fails

1. Read the message in the app.
2. Open **Diagnostics**.
3. Click **Copy Log** or **Export Log to File...**.
4. Send the log and the failed test number to the developer.

The two-instance test does not replace later testing with separate staff accounts, separate Outlook profiles, and a test network share.
