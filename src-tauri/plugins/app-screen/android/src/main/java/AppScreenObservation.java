package com.takosumi.appscreen;

import java.util.concurrent.atomic.AtomicInteger;

/** Debug observation only: primitive metadata, no Android/View APIs or authority. */
public final class AppScreenObservation {
    public static final String TAG = "TakosWebHomeNative";
    public static final int LIMIT = 64;
    public static final int MAX_LINE = 320;
    public enum Stage {
        BACK_ENTER, BACK_RETURN, BIND_ENTER, ADD_RETURN, BIND_RETURN,
        UNBIND_ENTER, REMOVE_RETURN, UNBIND_RETURN, RESUME, PAUSE,
        PAGE_START, PAGE_COMMIT, PAGE_FINISH, RENDER_GONE, CAP_EXHAUSTED
    }
    public enum Logical { UNKNOWN, ADD_RETURN, REMOVE_RETURN }
    public interface Clock { long now(); }
    public interface Sink { void write(String line); }
    private final boolean enabled;
    private final int pid;
    private final Clock clock;
    private final Sink sink;
    private final AtomicInteger attempts = new AtomicInteger();

    public AppScreenObservation(boolean enabled, int pid, Clock clock, Sink sink) {
        this.enabled = enabled;
        this.pid = pid;
        this.clock = clock;
        this.sink = sink;
    }

    public void record(Stage stage, int view, boolean currentEntry, boolean host,
                       long generation, boolean visible, boolean resumed, boolean evicted,
                       Logical logical) {
        if (!enabled) return;
        int before = attempts.get();
        // One nonblocking attempt; contention drops metadata, never retries UI work.
        if (before >= LIMIT || !attempts.compareAndSet(before, before + 1)) return;
        int seq = before + 1; // Failed observations also consume the cap.
        try {
            boolean capped = seq == LIMIT;
            Stage actual = capped ? Stage.CAP_EXHAUSTED : stage;
            Logical attachment = capped ? Logical.UNKNOWN : logical;
            long elapsed = clock.now();
            if (elapsed < 0 || pid <= 0) return;
            String line = "v=1 seq=" + seq + " pid=" + pid + " t=" + elapsed
                + " stage=" + actual.name() + " view=" + view
                + " cur=" + (currentEntry ? 1 : 0) + " host=" + (host ? 1 : 0)
                + " gen=" + generation + " vis=" + (visible ? 1 : 0)
                + " res=" + (resumed ? 1 : 0) + " evict=" + (evicted ? 1 : 0)
                + " logical=" + attachment.name() + " physical=unknown cap=" + (capped ? 1 : 0);
            if (line.length() <= MAX_LINE) sink.write(line);
        } catch (Throwable ignored) {
            // Logging/clock/cap failure must never replace navigation or its exception.
        }
    }
}
