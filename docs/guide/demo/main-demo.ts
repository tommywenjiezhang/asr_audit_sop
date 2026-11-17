// Entry point for the user-guide demo: the real app UI on top of a fake backend with fictional data.
import { mockIPC } from "@tauri-apps/api/mocks";
import { mount } from "svelte";
import { handler } from "./mock";
import "../../../src/app.css";
import App from "../../../src/App.svelte";

mockIPC((cmd, args) => handler(cmd, args));
// the clipboard works in the screenshots without a permission prompt
Object.defineProperty(navigator, "clipboard", { value: { writeText: async () => {} }, configurable: true });

mount(App, { target: document.getElementById("app")! });

// small, unobtrusive marker so screenshots are never mistaken for real data
const tag = document.createElement("div");
tag.textContent = "Demo data — fictional";
tag.style.cssText = "position:fixed;right:8px;bottom:6px;font:11px 'Segoe UI',sans-serif;color:#8a6d00;background:#fff8e1;border:1px solid #f0d58a;border-radius:4px;padding:1px 6px;z-index:100;pointer-events:none";
document.body.appendChild(tag);
