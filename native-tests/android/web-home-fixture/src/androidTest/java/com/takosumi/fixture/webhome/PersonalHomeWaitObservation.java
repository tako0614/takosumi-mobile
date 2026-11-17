package com.takosumi.fixture.webhome;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Metadata only. Neither a query oracle nor proof of the root searched by UiDevice. */
final class PersonalHomeWaitObservation {
    static final int SAMPLE_LIMIT = 256, CONTEXT_LIMIT = 4, WINDOW_LIMIT = 4;
    interface Snapshot { Capture read() throws Exception; }
    interface Sink { void put(String key, Object value) throws Exception; }
    static final class Capture {
        final long startedAt, finishedAt;
        final Window[] windows;
        Capture(long startedAt, long finishedAt, Window[] windows) {
            this.startedAt = startedAt;
            this.finishedAt = finishedAt;
            this.windows = windows;
        }
    }
    static final class Window {
        final int windowId, rootWindowId, type;
        final boolean active, focused, packageMatches;
        Window(int windowId, int rootWindowId, int type, boolean active, boolean focused, boolean packageMatches) {
            this.windowId = windowId;
            this.rootWindowId = rootWindowId;
            this.type = type;
            this.active = active;
            this.focused = focused;
            this.packageMatches = packageMatches;
        }
    }

    boolean waiting = true, incomplete, capped, contextCapped, returned;
    int samples, queryNull, disabled, emptyBounds, visible, late, dropped, contextsDropped;
    long started = -1, deadline = -1, finished = -1, firstAt = -1, lastAt = -1;
    long nextContextAt = -1;
    BackWaitObservation.Outcome first, last;
    final List<Map<String, Object>> contexts = new ArrayList<>();

    void begin(long start, long end) {
        if (started != -1 || start < 0 || end < start) { incomplete = true; return; }
        started = start;
        deadline = end;
        nextContextAt = start;
    }

    void finish(long now, boolean didReturn) {
        finished = now;
        returned = didReturn;
        waiting = false;
        if (started < 0 || now < started) incomplete = true;
    }

    void sample(BackWaitObservation.Outcome outcome, long now, Snapshot snapshot) {
        if (!waiting) return;
        if (outcome == null || started < 0 || now < started || now < lastAt) { incomplete = true; return; }
        if (now > deadline) { if (late < SAMPLE_LIMIT) late++; incomplete = true; return; }
        boolean changed = last != outcome;
        if (first == null) { first = outcome; firstAt = now; }
        last = outcome;
        lastAt = now;
        if (samples == SAMPLE_LIMIT) {
            capped = true;
            if (dropped < SAMPLE_LIMIT) dropped++;
            return;
        }
        samples++;
        switch (outcome) {
            case QUERY_NULL: queryNull++; break;
            case DISABLED: disabled++; break;
            case EMPTY_BOUNDS: emptyBounds++; break;
            case VISIBLE: visible++; break;
        }
        if (!changed && now < nextContextAt) return;
        if (contexts.size() == CONTEXT_LIMIT) {
            contextCapped = true;
            if (contextsDropped < SAMPLE_LIMIT) contextsDropped++;
            return;
        }
        Map<String, Object> context = new LinkedHashMap<>();
        context.put("sample", samples);
        context.put("queryReturnedAtMs", now);
        context.put("outcome", outcome.name());
        // UiObject2 in the pinned2.3.0 API does not expose its node/window.
        context.put("querySelectedRoot", "unknown");
        context.put("relation", "post_query_window_context");
        List<Map<String, Object>> windows = new ArrayList<>();
        String state = "returned";
        boolean truncated = false;
        try {
            Capture capture = snapshot.read();
            Window[] values = capture == null ? null : capture.windows;
            if (capture != null) {
                context.put("captureStartedAtMs", capture.startedAt);
                context.put("captureFinishedAtMs", capture.finishedAt);
            }
            if (capture != null && (capture.startedAt < now || capture.finishedAt < capture.startedAt
                    || capture.finishedAt > deadline)) { state = "outside_deadline"; incomplete = true; }
            else if (values == null) { state = "unknown"; incomplete = true; }
            else {
                truncated = values.length > WINDOW_LIMIT;
                for (int i = 0; i < Math.min(values.length, WINDOW_LIMIT); i++) {
                    Window value = values[i];
                    if (value == null || value.windowId < 0 || value.rootWindowId < -1) {
                        incomplete = true; state = "partial"; continue;
                    }
                    Map<String, Object> row = new LinkedHashMap<>();
                    row.put("windowId", value.windowId);
                    row.put("rootWindowId", value.rootWindowId < 0 ? null : value.rootWindowId);
                    row.put("rootWindowAgrees", value.rootWindowId < 0 ? null : value.windowId == value.rootWindowId);
                    row.put("type", value.type >= 1 && value.type <= 5 ? value.type : 0);
                    row.put("active", value.active);
                    row.put("focused", value.focused);
                    row.put("rootPackageMatchesTarget", value.rootWindowId < 0 ? null : value.packageMatches);
                    windows.add(row);
                }
            }
        } catch (Exception ignored) { state = "unavailable"; incomplete = true; }
        context.put("state", state);
        context.put("truncated", truncated);
        context.put("windows", windows);
        if (truncated) incomplete = true;
        contexts.add(context);
        // Comparisons inside the existing loop, not a timer or extra wait.
        long elapsed = now - started;
        nextContextAt = elapsed < 5000 ? started + 5000 : elapsed < 10000 ? started + 10000
            : elapsed < 14000 ? started + 14000 : deadline == Long.MAX_VALUE ? deadline : deadline + 1;
    }

    boolean writeTo(Sink sink, boolean observerIncomplete) {
        try {
            sink.put("schema", "takosumi.personal-home-wait-observation@v1");
            sink.put("diagnosticOnly", true);
            sink.put("phase", "post_reset_personal_home");
            sink.put("clockDomain", "android_elapsed_realtime");
            sink.put("sampleLimit", SAMPLE_LIMIT);
            sink.put("contextLimit", CONTEXT_LIMIT);
            sink.put("windowLimit", WINDOW_LIMIT);
            sink.put("waitStartedAtMs", started);
            sink.put("waitDeadlineAtMs", deadline);
            sink.put("waitFinishedAtMs", finished);
            sink.put("waitReturned", returned);
            sink.put("samples", samples);
            sink.put("queryNull", queryNull);
            sink.put("disabled", disabled);
            sink.put("emptyBounds", emptyBounds);
            sink.put("visible", visible);
            sink.put("firstOutcome", first == null ? "unknown" : first.name());
            sink.put("lastOutcome", last == null ? "unknown" : last.name());
            sink.put("firstAtMs", firstAt);
            sink.put("lastAtMs", lastAt);
            sink.put("lateSamplesCapped", late);
            sink.put("droppedCapped", dropped);
            sink.put("capReached", capped);
            sink.put("contextCapReached", contextCapped);
            sink.put("contextsDroppedCapped", contextsDropped);
            sink.put("recordingIncomplete", observerIncomplete || incomplete || capped || contextCapped);
            sink.put("contexts", contexts);
            return true;
        } catch (Exception ignored) { incomplete = true; return false; }
    }
}
