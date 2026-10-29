// Structured logging for the relay.
//
// What may be logged is decided here, by type: field values are short scalars
// chosen by relay code. Message bodies, device tokens, device secrets, VAPID
// JWTs (and therefore the VAPID `sub`), Authorization headers and app origins
// are never passed to a logger. The tests capture every line and check this.

export type LogValue = string | number | boolean | null;
export type LogFields = Record<string, LogValue>;

export interface RelayLog {
  info(event: string, fields?: LogFields): void;
  warn(event: string, fields?: LogFields): void;
  error(event: string, fields?: LogFields): void;
}

function emit(level: "info" | "warn" | "error", event: string, fields?: LogFields) {
  const line = JSON.stringify({ level, event, ...fields });
  if (level === "error") console.error(line);
  else if (level === "warn") console.warn(line);
  else console.log(line);
}

export const consoleLog: RelayLog = {
  info: (event, fields) => emit("info", event, fields),
  warn: (event, fields) => emit("warn", event, fields),
  error: (event, fields) => emit("error", event, fields),
};

/** A short, non-identifying description of an unexpected error. */
export function errorKind(error: unknown): string {
  if (error instanceof Error) return error.name || "Error";
  return typeof error;
}
