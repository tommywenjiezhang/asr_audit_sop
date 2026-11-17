# Template Coverage Report

Source: `SOP for ASR-Audit Problems.docx` (all sections, links and figures reviewed). Machine-readable catalog: `templates/catalog.json`.

**Status legend:** **verbatim** = SOP wording kept; the only changes are a `{{contact_first_name}}` greeting and named placeholders where the SOP had a variable slot. **adapted** = sentences come from the SOP but were split or combined. **review_required** = the SOP has no complete email for this case, so the starter uses only SOP-supported statements.

| Issue type | Template (file) | SOP section | Page | Status |
|---|---|---|---|---|
| School needs NJIIS enrollment | enrollment_instructions | SCHOOL NEEDS ENROLLMENT | 3 | verbatim |
| Missing school subtype | request_subtype | SCHOOL SUBTYPE NEEDED / Subject: School Subtype Error | 3 | verbatim |
| Missing school subtype | subtype_received_pending | SCHOOL SUBTYPE NEEDED ("Once I get that info, I will update NJIIS") | 3 | review_required |
| Missing subtype / Subtype updated | confirm_subtype_updated | REPLY ONCE RECEIVING SUBTYPE | 3 | verbatim |
| ASR training needed | asr_training/training_instructions | NEEDS TRAINING – ASR TRAINING | 4 | verbatim |
| ASR training / Training completed | asr_training/access_followup | ASR + LHD TRAINING (activation wait, 2 business days) | 4 | review_required |
| LHD Audit training | lhd_training/training_instructions | NEEDS TRAINING – LHD TRAINING | 4 | verbatim |
| LHD Audit training | lhd_training/access_followup | LHD TRAINING (adapted) | 4 | review_required |
| New user account | request_information_with_training | PERSON NEEDS NEW USER ACCOUNT AND TAKE THE ASR TRAINING | 5 | verbatim (field list → `{{account_details_needed}}`) |
| New user account | request_information | CREATE NEW USER | 15 | verbatim |
| New user account / created | account_created | CREATED NEW ACCOUNT | 14 | verbatim |
| Completed training – access | access_activated | COMPLETED TRAINING AND NEED ACCESS TO SCHOOL ("SCHOOL" → `{{school_name}}`) | 6 | verbatim |
| School Forms tab not visible | missing_tab (figure image3) | IF THE PERSON HAS TAKEN THE TRAINING BUT CANNOT FIND THE SCHOOL FORMS TAB | 6 | verbatim |
| LHD access denied | change_current_facility | LHDS GETTING ACCESS DENIED AT THE SCHOOL REPORT | 7 | verbatim |
| LHD access denied | select_school_dropdown (figure image10) | LHD GET ACCESS DENIED | 14 | verbatim (manual choice only) |
| LHD access denied | access_followup | LHD GET ACCESS DENIED (request details) | 14 | review_required |
| Webinar black screen | clear_cache (figure image8) | REPLY TO USERS WHO … SEE A BLACK SCREEN | 12–13 | verbatim |
| Webinar black screen | browser_it_troubleshooting | same section, steps 2–3 | 12–13 | adapted |
| Save form as PDF | save_as_pdf (figures image4–7) | SAVING COPY OF ASR/AUDIT FORM | 8–11 | verbatim |
| General enrollment & ASR process | asr_process (deadline → `{{asr_deadline}}`) | FULL SUMMARY OF SCHOOL ENROLLMENT & ASR TRAINING PROCESS | 8 | verbatim |
| Confirm receipt / not submitted / grade problem | submission_confirmed (figures image16, 17; "[LIST GRADES HERE]" → `{{submitted_grades}}`) | CONFIRMING RECEIPT OF DATA – data found | 16–17 | verbatim |
| same | submission_not_found | CONFIRMING RECEIPT OF DATA – data NOT found | 17 | verbatim |
| Grade-specific problem | grades_partially_submitted | data found + verified `{{missing_grades}}` | 16–17 | adapted |
| same | verification_pending | no interim wording in the SOP | 16 | review_required |
| General user guide | user_guide | LINKING THE USER GUIDE | 8 | verbatim |
| Other / Unclassified | manual_review | none | – | review_required |

## SOP content that is not an email

- **SOP FOR ANSWERING EMAILS (p.1):** workflow rules such as "close the loop", "mark complete" and "move to Completed". The app never moves or flags mail, so staff still do these steps by hand.
- **STEPS FOR ADMIN STAFF TO CHECK FOR DATA SUBMISSION (p.16):** stored as `internal_steps` on the submission issue types. They are shown to staff only and never put in emails.

## Notes for review

- **Figure image9** (Chrome "Delete browsing data" dialog) shows a staff member's personal Gmail address. It is left out of `clear_cache.html` on purpose. Add it back only after the screenshot is redacted.
- **Webinar links differ:** the SOP's "Full Summary" section links straight to GoToWebinar, while the training sections use the NJIIS Training page. Both are kept as written.
- **ASR deadline:** "Dec 1, 2026 at 11:59 PM" is a setting, not hard-coded, so it can be updated each year.
