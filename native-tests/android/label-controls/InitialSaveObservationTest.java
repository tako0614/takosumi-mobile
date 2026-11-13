package com.takosumi.fixture.webhome;

import java.io.IOException;
import java.util.HashMap;
import java.util.Map;
import java.util.Set;

/** Existing-JDK-only controls; no Android, clock, AX, target or network. */
public final class InitialSaveObservationTest {
    private static int checks;
    private static void check(boolean ok) { checks++; if (!ok) throw new AssertionError("initial outcome control " + checks); }
    public static void main(String[] args) {
        InitialSaveObservation value = new InitialSaveObservation();
        value.sample(BackWaitObservation.Outcome.VISIBLE);
        check(value.samples == 0 && value.first == null && value.last == null);
        Map<String, Object> absent = new HashMap<>();
        check(value.writeTo(absent::put, false));
        check(absent.get("firstOutcome").equals("unknown") && absent.get("lastOutcome").equals("unknown"));
        value.waiting = true;
        value.sample(BackWaitObservation.Outcome.QUERY_NULL);
        value.sample(BackWaitObservation.Outcome.DISABLED);
        value.sample(BackWaitObservation.Outcome.EMPTY_BOUNDS);
        value.sample(BackWaitObservation.Outcome.VISIBLE);
        check(value.samples == 4 && value.queryNull == 1 && value.disabled == 1 && value.emptyBounds == 1 && value.visible == 1);
        check(value.first == BackWaitObservation.Outcome.QUERY_NULL && value.last == BackWaitObservation.Outcome.VISIBLE);
        value.waiting = false;
        value.sample(BackWaitObservation.Outcome.DISABLED);
        check(value.samples == 4 && value.last == BackWaitObservation.Outcome.VISIBLE);
        Map<String, Object> metadata = new HashMap<>();
        check(value.writeTo(metadata::put, false));
        check(metadata.keySet().equals(Set.of("schema", "diagnosticOnly", "phase", "sampleLimit",
            "recordingIncomplete", "samples", "queryNull", "disabled", "emptyBounds", "visible",
            "firstOutcome", "lastOutcome", "capReached", "droppedCapped")));
        check(metadata.get("diagnosticOnly").equals(true) && metadata.get("phase").equals("initial_save_wait"));
        check(metadata.get("recordingIncomplete").equals(false) && metadata.get("firstOutcome").equals("QUERY_NULL"));
        Map<String, Object> refusedObserver = new HashMap<>();
        check(value.writeTo(refusedObserver::put, true) && refusedObserver.get("recordingIncomplete").equals(true));

        InitialSaveObservation capped = new InitialSaveObservation();
        capped.waiting = true;
        for (int i = 0; i < InitialSaveObservation.LIMIT + 1000; i++) capped.sample(BackWaitObservation.Outcome.QUERY_NULL);
        check(capped.samples == InitialSaveObservation.LIMIT && capped.queryNull == InitialSaveObservation.LIMIT);
        check(capped.dropped == InitialSaveObservation.LIMIT && capped.capReached);
        capped.sample(BackWaitObservation.Outcome.VISIBLE);
        check(capped.visible == 0 && capped.last == BackWaitObservation.Outcome.VISIBLE && capped.first == BackWaitObservation.Outcome.QUERY_NULL);
        Map<String, Object> saturated = new HashMap<>();
        check(capped.writeTo(saturated::put, false) && saturated.get("recordingIncomplete").equals(true));
        InitialSaveObservation missing = new InitialSaveObservation();
        missing.waiting = true; missing.sample(null);
        check(missing.incomplete && missing.samples == 0 && missing.first == null && missing.last == null);
        int[] calls = {0};
        check(InitialSaveObservation.safely(() -> calls[0]++));
        check(!InitialSaveObservation.safely(() -> { calls[0]++; throw new IllegalStateException("private observer failure"); }));
        check(calls[0] == 2);
        int[] writes = {0};
        check(!value.writeTo((key, item) -> { writes[0]++; throw new IOException("private sink failure"); }, false));
        check(writes[0] == 1 && value.incomplete);
        AssertionError original = new AssertionError("original wait failure");
        AssertionError raised = null;
        int[] cleanup = {0};
        try {
            try { throw original; }
            finally {
                InitialSaveObservation.safely(() -> { throw new IllegalStateException("private observer failure"); });
                value.writeTo((key, item) -> { throw new IOException("private sink failure"); }, false);
                cleanup[0]++;
            }
        } catch (AssertionError error) { raised = error; }
        check(raised == original && cleanup[0] == 1);
        System.out.println("PASS: initial save outcome metadata checks=" + checks);
    }
}
