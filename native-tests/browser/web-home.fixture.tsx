// Real shared UI and request adapter, with synthetic preferences/IPC only.
import { render } from "solid-js/web";
import { WebHome } from "../../src/web-home.tsx";
import { CONNECTIONS_KEY, type WebConnection } from "../../src/web-connections.ts";
import { clearWebProfileData } from "../../src/web-profile-data.ts";
import "../../src/web-home.css";

const initial: WebConnection[] = [
  { name: "Work", origin: "https://sumi.example", profile: "11111111-1111-4111-8111-111111111111" },
  { name: "Personal", origin: "https://sumi.example", profile: "22222222-2222-4222-8222-222222222222" },
  { name: "Legacy", origin: "https://legacy.example", profile: "shared" },
];
const preferences = new Map([[CONNECTIONS_KEY, JSON.stringify(initial)]]);
const opened: WebConnection[] = [];
const clears: { command: string; args: unknown }[] = [];
let pending: { resolve: (value: unknown) => void; reject: (reason: unknown) => void } | undefined;
const fixture = {
  snapshot: () => ({ opened, clears, saved: JSON.parse(preferences.get(CONNECTIONS_KEY)!) }),
  finish(value: unknown) {
    const operation = pending;
    if (!operation) throw new Error("No pending fixture clear");
    pending = undefined;
    operation.resolve(value);
  },
  fail(reason: string) {
    const operation = pending;
    if (!operation) throw new Error("No pending fixture clear");
    pending = undefined;
    operation.reject(reason);
  },
};
declare global { interface Window { resetFixture: typeof fixture } }
window.resetFixture = fixture;
render(() => <WebHome
  storage={{ getItem: (key) => preferences.get(key) ?? null, setItem: (key, value) => { preferences.set(key, value); } }}
  open={async (connection) => { opened.push(connection); }}
  clearData={(connection) => clearWebProfileData(connection, <T,>(command: string, args?: Record<string, unknown>) => {
    if (pending) return Promise.reject(new Error("Duplicate fixture operation"));
    clears.push({ command, args });
    return new Promise<T>((resolve, reject) => { pending = { resolve: (value) => resolve(value as T), reject }; });
  })}
/>, document.getElementById("root")!);
