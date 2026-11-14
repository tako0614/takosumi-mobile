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
        if (args.length == 1 && args[0].equals("entry")) {
            entryAdmission();
            return;
        }
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
        entryAdmission();
    }

    private static void entryAdmission() {
        int before = checks;
        Object personal = new Object(), work = new Object();
        PageBaseline page = new PageBaseline(personal, ORIGIN);
        PageBaseline.Entry entry = page.new Entry();
        // The saved ordering: an offscreen CTA exists before this same Page's
        // successful stop. Rejecting that early scroll must leave its budget.
        page.started(personal, 2);
        check(!entry.admit(false, personal, personal, 2, ORIGIN));
        check(!entry.scrolled() && !entry.tapped());
        page.stopped(personal, 2, true);
        check(entry.admit(false, personal, personal, 2, ORIGIN));
        check(entry.scrolled() && !entry.tapped());
        check(!entry.admit(false, personal, personal, 2, ORIGIN));
        check(page.capture(personal, personal, 2, ORIGIN, false) == null);
        check(entry.admit(true, personal, personal, 2, ORIGIN));
        check(!entry.admit(true, personal, personal, 2, ORIGIN));
        check(!entry.admit(false, personal, personal, 2, ORIGIN));
        check(page.capture(personal, personal, 2, ORIGIN, true) != null);

        // Ordinary Work entry retains one scroll then one tap, and a direct
        // visible CTA may consume its tap without requiring an extra scroll.
        PageBaseline workPage = new PageBaseline(work, ORIGIN);
        workPage.started(work, 3);
        workPage.stopped(work, 3, true);
        PageBaseline.Entry normal = workPage.new Entry();
        check(normal.admit(false, work, work, 3, ORIGIN));
        check(normal.admit(true, work, work, 3, ORIGIN));
        check(!normal.admit(false, work, work, 3, ORIGIN));
        check(!normal.admit(true, work, work, 3, ORIGIN));
        PageBaseline.Entry direct = workPage.new Entry();
        check(direct.admit(true, work, work, 3, ORIGIN));
        check(!direct.admit(false, work, work, 3, ORIGIN));

        PageBaseline.Entry denied = page.new Entry();
        check(!denied.admit(false, work, personal, 2, ORIGIN));
        check(!denied.admit(false, personal, work, 2, ORIGIN));
        check(!denied.admit(false, null, personal, 2, ORIGIN));
        check(!denied.admit(false, personal, null, 2, ORIGIN));
        check(!denied.admit(false, personal, personal, 0, ORIGIN));
        check(!denied.admit(false, personal, personal, 2, "about:blank"));
        check(!denied.admit(false, personal, personal, 2, null));
        page.stopped(personal, 2, false);
        check(!denied.admit(false, personal, personal, 2, ORIGIN));
        check(!denied.scrolled() && !denied.tapped());
        page.started(personal, 3);
        page.stopped(personal, 2, true);
        check(!denied.admit(false, personal, personal, 3, ORIGIN));
        page.stopped(personal, 3, true);
        check(denied.admit(false, personal, personal, 3, ORIGIN));
        // A later identity/navigation change cannot authorize the remaining tap.
        check(!denied.admit(true, work, personal, 3, ORIGIN));
        check(!denied.admit(true, personal, work, 3, ORIGIN));
        page.started(personal, 4);
        check(!denied.admit(true, personal, personal, 4, ORIGIN));
        check(!denied.tapped());
        page.stopped(personal, 4, true);
        check(denied.admit(true, personal, personal, 4, ORIGIN));
        check(!denied.admit(false, personal, personal, 4, ORIGIN));
        System.out.println("PASS: Gecko entry gesture admission checks=" + (checks - before));
    }
}
