// Real shared UI and request adapter, with synthetic preferences/IPC only.
import { render } from "solid-js/web";
import { createSignal } from "solid-js";
import { WebHome } from "../../src/web-home.tsx";
import { CONNECTIONS_KEY, type WebConnection } from "../../src/web-connections.ts";
import { clearWebProfileData } from "../../src/web-profile-data.ts";
import type { PendingPermission, PermissionAnswer, WebPermissionPrompts } from "../../src/web-permissions.ts";
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
const [permissionPrompts, setPermissionPrompts] = createSignal<WebPermissionPrompts>();
let permissionListener: ((list: readonly PendingPermission[]) => void) | undefined;
let permissionList: ((list: readonly PendingPermission[]) => void) | undefined;
let permissionAnswer: { resolve: (list: readonly PendingPermission[]) => void; reject: (reason: unknown) => void } | undefined;
const answers: { prompt: PendingPermission; decision: PermissionAnswer }[] = [];
let unlistened = 0;
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
  permissions: {
    begin() {
      if (permissionPrompts() || permissionList || permissionAnswer) throw new Error("Pending permission fixture");
      const surface: WebPermissionPrompts = {
        list: () => new Promise((resolve) => { permissionList = resolve; }),
        answer: (prompt, decision) => {
          answers.push({ prompt, decision });
          return new Promise((resolve, reject) => { permissionAnswer = { resolve, reject }; });
        },
        watch: async (listener) => {
          permissionListener = listener;
          return () => { unlistened += 1; };
        },
      };
      setPermissionPrompts(surface);
    },
    snapshot: () => ({ listing: Boolean(permissionList), answering: Boolean(permissionAnswer), answers, unlistened }),
    emit(list: readonly PendingPermission[]) { permissionListener?.(list); },
    finishList(list: readonly PendingPermission[]) {
      if (!permissionList) throw new Error("No pending permission list");
      const resolve = permissionList;
      permissionList = undefined;
      resolve(list);
    },
    finishAnswer(list: readonly PendingPermission[]) {
      if (!permissionAnswer) throw new Error("No pending permission answer");
      const operation = permissionAnswer;
      permissionAnswer = undefined;
      operation.resolve(list);
    },
    failAnswer() {
      if (!permissionAnswer) throw new Error("No pending permission answer");
      const operation = permissionAnswer;
      permissionAnswer = undefined;
      operation.reject(new Error("Synthetic permission transport refusal"));
    },
    end() { setPermissionPrompts(undefined); },
  },
};
declare global { interface Window { resetFixture: typeof fixture } }
window.resetFixture = fixture;
render(() => <WebHome
  permissionPrompts={permissionPrompts()}
  storage={{ getItem: (key) => preferences.get(key) ?? null, setItem: (key, value) => { preferences.set(key, value); } }}
  open={async (connection) => { opened.push(connection); }}
  clearData={(connection) => clearWebProfileData(connection, <T,>(command: string, args?: Record<string, unknown>) => {
    if (pending) return Promise.reject(new Error("Duplicate fixture operation"));
    clears.push({ command, args });
    return new Promise<T>((resolve, reject) => { pending = { resolve: (value) => resolve(value as T), reject }; });
  })}
/>, document.getElementById("root")!);
