package com.takosumi.fixture.geckodownload;

/** Exercises the actual admission state, without Gecko, AX or native threads. */
public final class PageBaselineTest {
    private static int checks;
    private static final String ORIGIN = "https://fixture.invalid/";
    private static void check(boolean value) {
        checks++;
        if (!value) throw new AssertionError("Page baseline check " + checks);
    }

    public static void main(String[] args) {
        Object personal = new Object(), work = new Object();
        PageBaseline baseline = new PageBaseline(personal, ORIGIN);
        // The failed native cohort had a zero initial baseline even with File
        // visible. Attach plus Work's retained AX alone is never sufficient.
        check(baseline.capture(personal, personal, 0, ORIGIN, true) == null);
        baseline.stopped(personal, 0, true);
        check(baseline.capture(personal, personal, 0, ORIGIN, true) == null);
        baseline.started(work, 1);
        baseline.stopped(work, 1, true);
        check(baseline.capture(personal, personal, 1, ORIGIN, true) == null);
        baseline.started(personal, 1);
        check(baseline.capture(personal, personal, 1, ORIGIN, true) == null);
        baseline.stopped(work, 1, true);
        check(baseline.capture(personal, personal, 1, ORIGIN, true) == null);
        baseline.stopped(personal, 1, false);
        check(baseline.capture(personal, personal, 1, ORIGIN, true) == null);
        baseline.stopped(personal, 1, true);
        check(baseline.capture(work, personal, 1, ORIGIN, true) == null);
        check(baseline.capture(personal, work, 1, ORIGIN, true) == null);
        check(baseline.capture(null, personal, 1, ORIGIN, true) == null);
        check(baseline.capture(personal, null, 1, ORIGIN, true) == null);
        check(baseline.capture(personal, personal, 1, ORIGIN, false) == null);
        check(baseline.capture(personal, personal, 1, null, true) == null);
        check(baseline.capture(personal, personal, 1, "about:blank", true) == null);
        check(baseline.capture(personal, personal, 2, ORIGIN, true) == null);
        String uri = ORIGIN + "editor";
        PageBaseline.Snapshot first = baseline.capture(personal, personal, 1, uri, true);
        check(first != null && first.generation == 1 && first.uri.equals(uri));
        // Baselines cannot be updated by subsequent navigation. The existing
        // return assertion must still see a real generation/URI difference.
        baseline.started(personal, 2);
        check(baseline.capture(personal, personal, 2, uri, true) == null);
        baseline.stopped(personal, 1, true);
        check(baseline.capture(personal, personal, 2, uri, true) == null);
        baseline.stopped(personal, 2, true);
        PageBaseline.Snapshot second = baseline.capture(personal, personal, 2, ORIGIN, true);
        check(second != null && first != second);
        check(first.generation == 1 && first.uri.equals(uri));
        check(first.generation != second.generation && !first.uri.equals(second.uri));
        // Another Page's successful session cannot initialize this Page.
        PageBaseline other = new PageBaseline(work, ORIGIN);
        other.started(work, 7);
        other.stopped(work, 7, true);
        check(other.capture(personal, personal, 7, ORIGIN, true) == null);
        check(baseline.capture(personal, personal, 7, ORIGIN, true) == null);
        check(other.capture(work, work, 7, ORIGIN, true).generation == 7);
        // A failed current load revokes successful-stop admission.
        baseline.stopped(personal, 2, false);
        check(baseline.capture(personal, personal, 2, ORIGIN, true) == null);
        System.out.println("PASS: Gecko initial page baseline checks=" + checks);
    }
}
