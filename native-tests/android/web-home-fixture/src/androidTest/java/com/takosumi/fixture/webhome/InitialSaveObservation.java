package com.takosumi.fixture.webhome;

/** Count only already-read initial Save query outcomes; never UI readiness. */
final class InitialSaveObservation {
    static final int LIMIT = 256;
    boolean waiting, incomplete, capReached;
    int samples, queryNull, disabled, emptyBounds, visible, dropped;
    BackWaitObservation.Outcome first, last;

    static boolean safely(Runnable observer) {
        try { observer.run(); return true; }
        catch (RuntimeException ignored) { return false; }
    }

    void sample(BackWaitObservation.Outcome outcome) {
        if (!waiting) return;
        if (outcome == null) { incomplete = true; return; }
        if (first == null) first = outcome;
        last = outcome; // Retain the last delivered enum even after counts saturate.
        if (samples == LIMIT) {
            capReached = true;
            if (dropped < LIMIT) dropped++;
            return;
        }
        samples++;
        switch (outcome) {
            case QUERY_NULL: queryNull++; break;
            case DISABLED: disabled++; break;
            case EMPTY_BOUNDS: emptyBounds++; break;
            case VISIBLE: visible++; break;
        }
    }

    interface Sink { void put(String key, Object value) throws Exception; }

    boolean writeTo(Sink sink, boolean observerIncomplete) {
        try {
            sink.put("schema", "takosumi.initial-save-outcome-observation@v1");
            sink.put("diagnosticOnly", true);
            sink.put("phase", "initial_save_wait");
            sink.put("sampleLimit", LIMIT);
            sink.put("recordingIncomplete", observerIncomplete || incomplete || capReached);
            sink.put("samples", samples);
            sink.put("queryNull", queryNull);
            sink.put("disabled", disabled);
            sink.put("emptyBounds", emptyBounds);
            sink.put("visible", visible);
            sink.put("firstOutcome", first == null ? "unknown" : first.name());
            sink.put("lastOutcome", last == null ? "unknown" : last.name());
            sink.put("capReached", capReached);
            sink.put("droppedCapped", dropped);
            return true;
        } catch (Exception ignored) {
            incomplete = true;
            return false;
        }
    }
}
