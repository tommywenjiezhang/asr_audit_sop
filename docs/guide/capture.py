"""Capture the screenshots for the user guide (docs/guide/images/*.png).

Runs the real app UI in Microsoft Edge on FICTIONAL demo data (docs/guide/demo, mocked backend),
clicks through every step like a user, outlines the control to click in red, and saves an image per step.
Outlook/Copilot steps use the labelled illustrations in docs/guide/mockups/mockup.html.

    python docs/guide/capture.py          (from the project root; starts and stops the Vite docs server)
"""
from __future__ import annotations

import os
import subprocess
import sys
import time
import urllib.request

from playwright.sync_api import Locator, Page, sync_playwright

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "docs", "guide", "images")
BASE = "http://localhost:1430"
APP = BASE + "/docs/guide/demo/index.html"
MOCK = BASE + "/docs/guide/mockups/mockup.html"

MARK_JS = """([box, n, label]) => {
  const d = document.createElement('div');
  d.className = '__mark';
  d.style.cssText = `position:fixed;left:${box.x - 4}px;top:${box.y - 4}px;width:${box.width + 8}px;height:${box.height + 8}px;` +
    'border:3px solid #d32f2f;border-radius:6px;z-index:9999;pointer-events:none;box-shadow:0 0 0 3px rgba(211,47,47,.18)';
  const b = document.createElement('div');
  b.className = '__mark';
  b.textContent = n;
  const bx = Math.max(box.x - 16, 2), by = Math.max(box.y - 16, 2);
  b.style.cssText = `position:fixed;left:${bx}px;top:${by}px;width:24px;height:24px;border-radius:12px;background:#d32f2f;` +
    'color:#fff;font:bold 13px Segoe UI,sans-serif;display:flex;align-items:center;justify-content:center;z-index:10000;pointer-events:none';
  document.body.append(d, b);
  if (label) {
    const t = document.createElement('div');
    t.className = '__mark';
    t.textContent = label;
    t.style.cssText = 'position:fixed;background:#d32f2f;color:#fff;' +
      'font:600 12px Segoe UI,sans-serif;padding:3px 8px;border-radius:4px;z-index:10000;pointer-events:none;white-space:nowrap';
    document.body.append(t);
    // small targets: label to the right; large areas (or no room on the right): inside the bottom-right corner
    const inside = box.width > 420 || box.height > 300 || box.x + box.width + t.offsetWidth + 24 > window.innerWidth;
    t.style.left = (inside ? box.x + box.width - t.offsetWidth - 12 : box.x + box.width + 12) + 'px';
    t.style.top = (inside ? box.y + box.height - t.offsetHeight - 10 : box.y + box.height / 2 - t.offsetHeight / 2) + 'px';
  }
}"""


class Shooter:
    def __init__(self, page: Page):
        self.page = page
        self.count = 0

    def clear(self):
        self.page.evaluate("document.querySelectorAll('.__mark').forEach(e => e.remove())")

    def mark(self, loc: Locator, n: int | str, label: str = ""):
        loc.scroll_into_view_if_needed()
        box = loc.bounding_box()
        if box:
            self.page.evaluate(MARK_JS, [box, str(n), label])

    def shot(self, name: str, area: Locator | list[Locator] | None = None, pad: int = 14, pad_right: int = 160):
        self.page.wait_for_timeout(250)
        path = os.path.join(OUT, f"{name}.png")
        if area is None:
            self.page.screenshot(path=path)
        else:
            boxes = [b for b in (x.bounding_box() for x in (area if isinstance(area, list) else [area])) if b]
            x0 = max(min(b["x"] for b in boxes) - pad, 0)
            y0 = max(min(b["y"] for b in boxes) - pad, 0)
            x1 = min(max(b["x"] + b["width"] for b in boxes) + pad + pad_right, self.page.viewport_size["width"])
            y1 = min(max(b["y"] + b["height"] for b in boxes) + pad, self.page.viewport_size["height"])
            self.page.screenshot(path=path, clip={"x": x0, "y": y0, "width": x1 - x0, "height": y1 - y0})
        self.count += 1
        print("  saved", name)
        self.clear()


def panel(page: Page, heading: str) -> Locator:
    return page.locator(".panel", has=page.locator("h3", has_text=heading)).first


def section(page: Page, heading: str) -> Locator:
    return page.locator("section", has=page.locator("h3", has_text=heading)).first


def open_app(page: Page, scenario: str = ""):
    page.goto(APP + (f"?scenario={scenario}" if scenario else ""))
    page.wait_for_selector("text=T-000001", timeout=20000)


def open_ticket(page: Page, tid: str):
    page.locator(".item", has_text=tid).locator("button.body").click()
    page.wait_for_selector(f"h2:has-text('{tid}')")
    page.wait_for_timeout(300)


def run(page: Page):
    s = Shooter(page)
    # ------------------------------------------------------------ overview
    open_app(page)
    open_ticket(page, "T-000001")
    s.mark(page.locator("section.queue"), 1, "Ticket list")
    s.mark(page.locator(".actionbar"), 2, "Ticket workspace")
    s.shot("01-overview")

    # ------------------------------------------------------------ settings
    page.get_by_role("button", name="Settings", exact=True).click()
    page.wait_for_selector("text=Monitored Folders")
    page.wait_for_timeout(500)
    sec = section(page, "Outlook Connection")
    sec.get_by_role("button", name="Test Outlook Connection").click()
    page.wait_for_selector("text=Connected to Outlook")
    s.mark(sec.get_by_role("button", name="Test Outlook Connection"), 1)
    s.mark(sec.locator(".badge"), 2)
    s.shot("02-settings-test-connection", sec)

    mon = section(page, "Monitored Folders")
    mon.get_by_role("button", name="Find Mailboxes").click()
    mon.get_by_role("button", name="Help Desk Shared Mailbox (demo)").wait_for()
    s.mark(mon.get_by_role("button", name="Find Mailboxes"), 1)
    s.mark(mon.get_by_role("button", name="Help Desk Shared Mailbox (demo)"), 2)
    s.shot("03-settings-find-mailbox", mon.locator(".row.wrap").first)

    mon.get_by_role("button", name="Help Desk Shared Mailbox (demo)").click()
    mon.locator(".treebox").wait_for()
    inbox_btn = mon.locator(".treebox button.name", has_text="Inbox").first
    inbox_btn.click()
    s.mark(inbox_btn, 3, "Click Inbox")
    s.shot("04-settings-pick-inbox", mon.locator(".treebox"))

    mon.get_by_label("Include selected child folders").check()
    page.wait_for_timeout(300)
    my = mon.locator("label", has_text="My Folder").first
    if not my.locator("input").is_checked():
        my.locator("input").check()
    subs = mon.locator("label", has_text="and its subfolders").first
    if not subs.locator("input").is_checked():
        subs.locator("input").check()
    s.mark(mon.get_by_label("Include selected child folders"), 4)
    s.mark(my, 5)
    s.mark(subs, 6)
    s.mark(mon.locator(".notice"), 7, "Check this line")
    s.shot("05-settings-child-folders", [mon.get_by_text("Child folders", exact=True), mon.locator(".notice")])

    s.mark(mon.get_by_label("Unread items only"), 8)
    s.mark(mon.get_by_label("All items in the folder (recommended)"), 8)
    s.shot("06-settings-scan-mode", mon.locator(".field", has_text="Scanning never marks"))

    comp = section(page, "Completion Folder")
    comp.get_by_role("button", name="Open").click()
    comp.locator(".treebox").wait_for()
    done_btn = comp.locator(".treebox button.name", has_text="Completed").first
    done_btn.click()
    comp.get_by_role("button", name="Validate").click()
    comp.locator(".badge.ok").wait_for()
    s.mark(comp.get_by_role("button", name="Open"), 1)
    s.mark(done_btn, 2, "Click Completed")
    s.mark(comp.get_by_role("button", name="Validate"), 3)
    s.mark(comp.locator(".badge.ok"), 4)
    s.shot("07-settings-completion-folder", comp)

    st = section(page, "Staff Assignment")
    st.scroll_into_view_if_needed()
    s.mark(st.locator("table"), 1, "Name, email, folder")
    s.mark(st.get_by_role("button", name="+ Add staff member"), 2)
    s.mark(st.locator("select").first, 3)
    s.mark(st.locator("select").nth(1), 4)
    s.shot("36-staff-settings", st)
    st.get_by_role("button", name="Edit").first.click()
    st.get_by_role("button", name="Choose folder…").click()
    st.locator(".treebox").wait_for()
    s.mark(st.locator(".staffform input").nth(1), 1)
    s.mark(st.locator(".treebox button.name.sel"), 2, "Their folder")
    s.mark(st.get_by_role("button", name="Save staff member"), 3)
    s.shot("37-staff-edit", st.locator(".staffform"))
    st.get_by_role("button", name="Cancel").click()

    pr = section(page, "Copilot Prompt")
    s.mark(pr.locator("textarea"), 1, "Instructions for Copilot")
    s.mark(pr.get_by_role("button", name="Restore Default"), 2)
    s.shot("08-settings-prompt", pr)

    page.get_by_role("button", name="Save Settings").click()
    page.wait_for_selector(".toast")
    s.mark(page.get_by_role("button", name="Save Settings"), 1)
    s.mark(page.locator(".toast"), 2)
    s.shot("09-settings-save")

    # ------------------------------------------------------------ check for email
    page.get_by_role("button", name="Tickets", exact=True).click()
    page.wait_for_selector("text=T-000001")
    page.get_by_role("button", name="Check Now").click()
    page.wait_for_selector(".toast")
    s.mark(page.get_by_role("button", name="Check Now"), 1)
    s.mark(page.locator(".syncbox"), 2)
    s.mark(page.locator(".toast"), 3)
    s.shot("10-check-now")

    # ------------------------------------------------------------ find tickets
    search = page.get_by_placeholder("Search ticket, school")
    search.fill("0012345")
    page.wait_for_timeout(600)
    s.mark(search, 1, "Type a name, ID, phone…")
    s.mark(page.locator(".qhead .muted b"), 2)
    s.shot("11-search", page.locator("section.queue"))
    search.fill("")
    page.wait_for_timeout(500)

    q = page.locator(".qhead")
    q.get_by_role("button", name="More filters").click()
    q.locator("input[type=date]").nth(0).fill("2026-11-10")
    q.locator("input[type=date]").nth(1).fill("2026-11-14")
    page.evaluate("document.activeElement && document.activeElement.blur()")
    page.wait_for_timeout(600)
    s.mark(q.locator("select").first, 1)
    s.mark(q.locator("select").nth(1), 2)
    s.mark(q.locator(".dates"), 3)
    s.mark(q.get_by_role("button", name="Clear Filters"), 4)
    s.shot("12-filters", page.locator("section.queue"), pad_right=0)
    q.get_by_role("button", name="Clear Filters").click()
    q.get_by_role("button", name="Fewer filters").click()
    page.wait_for_timeout(500)

    # ------------------------------------------------------------ current tickets, scope, shortcuts
    s.mark(page.locator(".tabs"), 1)
    s.mark(page.locator(".scopebar"), 2)
    s.mark(q.get_by_role("button", name="My Tickets"), 3)
    s.mark(q.get_by_role("button", name="Unassigned"), 4)
    s.shot("38-current-tickets", [page.locator(".tabs"), q.get_by_role("button", name="My Tickets")], pad_right=0)

    # bulk selection
    page.locator(".item", has_text="T-000007").locator("input.pick").check()
    page.locator(".item", has_text="T-000008").locator("input.pick").check()
    s.mark(page.locator(".item", has_text="T-000007").locator("input.pick"), 1)
    s.mark(page.locator(".item", has_text="T-000008").locator("input.pick"), 2)
    s.mark(q.get_by_role("button", name="Assign selected (2)"), 3)
    s.shot("41-bulk-select", page.locator("section.queue"), pad_right=0)
    q.get_by_role("button", name="Clear", exact=True).click()

    # ------------------------------------------------------------ review a ticket
    open_ticket(page, "T-000001")
    ab = page.locator(".actionbar")
    s.mark(ab.locator("h2"), 1)
    s.mark(ab.locator(".badge").first, 2, "Status")
    s.shot("13-ticket-header", ab)

    open_ticket(page, "T-000002")
    iss = panel(page, "Issue")
    s.mark(iss.locator(".chip").first, 1, "Main issue")
    s.mark(iss.locator(".chip").nth(1), 2)
    s.mark(iss.locator("select").first, 3, "Add another issue")
    s.shot("14-issues", iss)

    open_ticket(page, "T-000001")
    snd = panel(page, "Sender")
    s.mark(snd, 1, "Correct anything that is wrong")
    s.shot("15-sender", snd)

    sch = panel(page, "School")
    addr = sch.locator(".field", has_text="School address").first
    s.mark(addr.locator(".badge"), 1, "Found in the email")
    verify = sch.locator("button", has_text="verify").first
    s.mark(verify, 2, "Click after you checked it")
    s.shot("16-field-labels", [sch.locator(".field").nth(0), addr])

    open_ticket(page, "T-000004")
    sch = page.locator(".panel", has=page.locator("h3", has_text="Schools")).first
    s.mark(sch.locator("b", has_text="School 1"), 1)
    s.mark(sch.locator("b", has_text="School 2"), 2)
    s.mark(sch.get_by_role("button", name="+ Add school"), 3)
    page.set_viewport_size({"width": 1440, "height": 1400})
    s.shot("17-two-schools", sch)
    page.set_viewport_size({"width": 1440, "height": 900})

    open_ticket(page, "T-000001")
    st = panel(page, "Staff")
    s.mark(st.locator("textarea").nth(0), 1, "What you already did")
    s.mark(st.locator("textarea").nth(1), 2, "Internal notes")
    s.shot("18-staff-actions", st)

    # ------------------------------------------------------------ copilot workflow
    ab.get_by_role("button", name="1. Copy Prompt").click()
    page.wait_for_selector("text=Prompt copied — paste it into Copilot.")
    s.mark(ab.get_by_role("button", name="1. Copy Prompt"), 1)
    s.mark(ab.locator(".status.ok"), 2)
    s.shot("19-copy-prompt", ab)

    pv = panel(page, "Copilot Prompt Preview")
    pv.scroll_into_view_if_needed()
    s.mark(pv.locator("pre.prompt"), 1, "Exactly what was copied")
    s.shot("20-prompt-preview", pv)

    ab.get_by_role("button", name="2. Open Outlook Reply").click()
    page.wait_for_selector("text=Outlook reply window opened")
    s.mark(ab.get_by_role("button", name="2. Open Outlook Reply"), 1)
    s.mark(ab.get_by_role("button", name="Reply All"), 2)
    s.mark(ab.locator(".status.ok").last, 3)
    s.shot("22-open-outlook-reply", ab)

    s.mark(ab.get_by_role("button", name="Copy Prompt & Open Outlook Reply"), 1, "Both in one click")
    s.shot("25-copy-and-open", ab)

    ab.get_by_role("button", name="3. Mark Done").click()
    page.wait_for_selector("text=flagged complete and moved")
    s.mark(ab.locator(".badge").first, 1, "Now Done")
    s.mark(ab.locator(".status.ok").last, 2)
    s.mark(ab.get_by_role("button", name="Reopen"), 3)
    s.shot("26-mark-done", ab)

    # Mark Done problem + Retry
    open_app(page, "markdone-error")
    open_ticket(page, "T-000005")
    ab = page.locator(".actionbar")
    ab.get_by_role("button", name="3. Mark Done").click()
    page.wait_for_selector("text=Not all Outlook steps succeeded")
    s.mark(ab.locator(".status.err"), 1)
    s.mark(ab.locator(".fails"), 2)
    s.mark(ab.get_by_role("button", name="3. Retry Mark Done"), 3)
    s.shot("28-mark-done-retry", ab)

    # Done ticket (in Ticket History): archived emails + Reopen
    open_app(page)
    page.locator(".tabs button", has_text="Ticket History").click()
    page.wait_for_selector(".item:has-text('T-000006')")
    s.mark(page.locator(".tabs button", has_text="Ticket History"), 1)
    s.mark(page.locator(".item", has_text="T-000003").locator(".reason"), 2)
    s.shot("44-ticket-history", page.locator("section.queue"), pad_right=0)
    open_ticket(page, "T-000006")
    ab = page.locator(".actionbar")
    em = page.locator(".panel", has=page.locator("h3", has_text="Emails")).first
    s.mark(ab.get_by_role("button", name="Reopen"), 1)
    s.shot("29-reopen", ab)
    s.mark(em.locator(".badge.ok").first, 1, "Moved to Completed")
    s.shot("30-archived-email", em)

    # original email not found -> fallback
    open_app(page, "not-found")
    open_ticket(page, "T-000007")
    ab = page.locator(".actionbar")
    ab.get_by_role("button", name="2. Open Outlook Reply").click()
    page.wait_for_selector("text=Open new message to sender")
    s.mark(ab.locator(".status.err"), 1)
    s.mark(ab.get_by_role("button", name="Open new message to sender"), 2)
    s.shot("31-email-not-found", ab)

    # ------------------------------------------------------------ assignment
    open_app(page)
    open_ticket(page, "T-000004")
    ab = page.locator(".actionbar")
    s.mark(ab.locator(".badge").first, 1, "Owner")
    s.mark(ab.get_by_role("button", name="Reassign"), 2)
    s.mark(ab.get_by_role("button", name="Start Work"), 3)
    s.shot("43-assign-buttons", ab)
    ab.get_by_role("button", name="Reassign").click()
    dlg = page.locator(".modal")
    dlg.locator("select").select_option(label="Teammate B")
    dlg.locator(".confirm").wait_for()
    dlg.locator("textarea").fill("Teammate B covers Mercer County schools.")
    page.wait_for_timeout(300)
    s.mark(dlg.locator("select"), 1)
    s.mark(dlg.locator(".confirm"), 2, "Check before assigning")
    s.mark(dlg.locator("textarea"), 3)
    s.mark(dlg.get_by_role("button", name="Assign 1 ticket"), 4)
    s.shot("39-assign-dialog", dlg, pad_right=0)
    dlg.get_by_role("button", name="Assign 1 ticket").click()
    dlg.locator("table").wait_for()
    s.mark(dlg.locator("table .badge").first, 1)
    s.shot("40-assign-result", dlg, pad_right=0)
    dlg.get_by_role("button", name="Close").click()

    open_ticket(page, "T-000002")
    asg = panel(page, "Assignment")
    asg.scroll_into_view_if_needed()
    s.mark(asg.locator(".badge").first, 1)
    s.mark(asg.get_by_role("button", name="Retry Notification"), 2)
    s.shot("42-notification-failed", asg)

    page.get_by_role("button", name="Team Overview").click()
    page.wait_for_selector("text=Reporting scope")
    s.shot("45-team-overview")

    # ------------------------------------------------------------ other pages
    open_app(page)
    page.get_by_role("button", name="+ New").click()
    page.wait_for_selector(".modal")
    s.mark(page.get_by_role("button", name="+ New"), 1)
    s.mark(page.locator(".modal"), 2)
    s.shot("32-new-ticket")
    page.get_by_role("button", name="Cancel").click()

    page.get_by_role("button", name="SOP Templates").click()
    page.wait_for_timeout(800)
    s.shot("33-sop-templates")

    page.get_by_role("button", name="Reports", exact=True).click()
    page.wait_for_selector("text=Issue Categories")
    s.mark(page.get_by_role("button", name="Export CSV (Excel-compatible)…"), 1)
    s.shot("34-reports")

    page.get_by_role("button", name="Settings", exact=True).click()
    page.wait_for_selector("text=Database / Backup")
    db = section(page, "Database / Backup")
    s.mark(db.get_by_role("button", name="Back Up Database…"), 1)
    s.mark(db.get_by_role("button", name="Restore From Backup…"), 2)
    s.shot("35-backup", db)

    # ------------------------------------------------------------ illustrations (Outlook / Copilot)
    page.set_viewport_size({"width": 1100, "height": 640})
    for name, view in [("00-start-menu", "start"), ("21-copilot", "copilot"), ("23-outlook-reply-window", "reply"), ("24-outlook-paste-send", "pasted"), ("27-outlook-completed", "completed")]:
        page.goto(f"{MOCK}?view={view}")
        page.wait_for_timeout(300)
        s.shot(name)
    return s.count


def main():
    os.makedirs(OUT, exist_ok=True)
    srv = subprocess.Popen("npx vite --config vite.docs.config.ts", cwd=ROOT, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(120):
            try:
                urllib.request.urlopen(APP, timeout=1)
                break
            except Exception:  # noqa: BLE001
                time.sleep(0.5)
        else:
            sys.exit("Vite docs server did not start on port 1430")
        with sync_playwright() as p:
            browser = p.chromium.launch(channel="msedge")
            page = browser.new_page(viewport={"width": 1440, "height": 900}, device_scale_factor=1.25, locale="en-US", timezone_id="America/New_York")
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            n = run(page)
            browser.close()
        print(f"{n} screenshots written to {OUT}")
        if errors:
            print("page errors:", *errors, sep="\n  ")
            sys.exit(1)
    finally:
        subprocess.run(f"taskkill /PID {srv.pid} /T /F", shell=True, capture_output=True)


if __name__ == "__main__":
    main()
