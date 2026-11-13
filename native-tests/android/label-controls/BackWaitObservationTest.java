package com.takosumi.fixture.webhome;

/** JDK-only controls: no Android, AX, timing sleep, target or network. */
public final class BackWaitObservationTest {
    private static int checks;
    private static void check(boolean ok) { checks++; if (!ok) throw new AssertionError("metadata control " + checks); }
    public static void main(String[] args) {
        BackWaitObservation value = new BackWaitObservation();
        check(value.first == null && value.last == null && value.firstAtMs == -1 && value.lastAtMs == -1);
        check(value.backStartedAtMs == -1 && value.waitStartedAtMs == -1 && !value.waitReturned);
        value.sample(BackWaitObservation.Outcome.VISIBLE, 1);
        check(value.samples == 0 && !value.waitReturned);
        value.beginBack(10); value.endBack(12, true); value.beginWait(13);
        value.sample(BackWaitObservation.Outcome.QUERY_NULL, 14);
        value.sample(BackWaitObservation.Outcome.DISABLED, 15);
        value.sample(BackWaitObservation.Outcome.EMPTY_BOUNDS, 16);
        value.sample(BackWaitObservation.Outcome.VISIBLE, 17);
        check(value.samples == 4 && value.queryNull == 1 && value.disabled == 1 && value.emptyBounds == 1 && value.visible == 1);
        check(value.first == BackWaitObservation.Outcome.QUERY_NULL && value.firstAtMs == 14);
        check(value.last == BackWaitObservation.Outcome.VISIBLE && value.lastAtMs == 17);
        check(!value.waitReturned && value.backReturned && value.backStartedAtMs == 10 && value.backFinishedAtMs == 12);
        value.endWait(18, true); value.sample(BackWaitObservation.Outcome.QUERY_NULL, 19);
        check(value.samples == 4 && value.lastAtMs == 17 && value.waitReturned && !value.waiting && value.waitFinishedAtMs == 18);

        BackWaitObservation failed = new BackWaitObservation();
        failed.beginBack(20); failed.endBack(21, false);
        check(!failed.backReturned && failed.waitStartedAtMs == -1 && failed.samples == 0);
        failed.beginWait(22); failed.sample(null, 23);
        check(failed.incomplete && failed.samples == 0 && failed.first == null);
        failed.sample(BackWaitObservation.Outcome.EMPTY_BOUNDS, 24); failed.endWait(15022, false);
        check(!failed.waitReturned && failed.emptyBounds == 1 && failed.visible == 0 && failed.lastAtMs == 24);

        BackWaitObservation capped = new BackWaitObservation(); capped.beginWait(30);
        for (int i = 0; i < BackWaitObservation.LIMIT + 1000; i++) capped.sample(BackWaitObservation.Outcome.QUERY_NULL, 31 + i);
        check(capped.samples == BackWaitObservation.LIMIT && capped.queryNull == BackWaitObservation.LIMIT);
        check(capped.dropped == BackWaitObservation.LIMIT && capped.capReached && capped.firstAtMs == 31);
        check(capped.lastAtMs == 30 + BackWaitObservation.LIMIT && !capped.waitReturned);
        capped.sample(BackWaitObservation.Outcome.VISIBLE, 9999); capped.endWait(10000, true);
        check(capped.visible == 0 && capped.waitReturned && capped.capReached && capped.lastAtMs == 30 + BackWaitObservation.LIMIT);
        System.out.println("PASS: back wait metadata checks=" + checks);
    }
}
