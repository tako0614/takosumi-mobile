package com.takosumi.fixture.webhome;

/** Bounded metadata from already-read query outcomes; never a readiness oracle. */
final class BackWaitObservation {
    static final int LIMIT = 256;
    enum Outcome { QUERY_NULL, DISABLED, EMPTY_BOUNDS, VISIBLE }
    long backStartedAtMs = -1, backFinishedAtMs = -1;
    long waitStartedAtMs = -1, waitFinishedAtMs = -1;
    long firstAtMs = -1, lastAtMs = -1;
    boolean backReturned, waitReturned, waiting, capReached, incomplete;
    int samples, queryNull, disabled, emptyBounds, visible, dropped;
    Outcome first, last;

    void beginBack(long now) { backStartedAtMs = now; }
    void endBack(long now, boolean returned) { backFinishedAtMs = now; backReturned = returned; }
    void beginWait(long now) { waitStartedAtMs = now; waiting = true; }
    void endWait(long now, boolean returned) {
        waitFinishedAtMs = now;
        waitReturned = returned;
        waiting = false;
    }

    void sample(Outcome outcome, long now) {
        if (!waiting) return;
        if (outcome == null) { incomplete = true; return; }
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
        if (first == null) { first = outcome; firstAtMs = now; }
        last = outcome;
        lastAtMs = now;
    }
}
