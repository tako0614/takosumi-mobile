package com.takosumi.fixture.geckodownload;

/** Initial editor admission only; all calls run on the session's main thread.
 * A completed navigation is not a document identity or a paint/AX guarantee. */
final class PageBaseline {
    private final Object session;
    private final String origin;
    private int started;
    private int completed;

    PageBaseline(Object session, String origin) {
        this.session = session;
        this.origin = origin;
    }

    void started(Object owner, int generation) {
        if (owner != session) return;
        started = generation;
        completed = 0;
    }

    void stopped(Object owner, int generation, boolean success) {
        if (owner != session) return;
        completed = success && generation > 0 && generation == started ? generation : 0;
    }

    Snapshot capture(Object selected, Object attached, int generation, String uri, boolean editorVisible) {
        if (selected != session || attached != session || !editorVisible || generation <= 0
                || started != generation || completed != generation || uri == null || !uri.startsWith(origin)) return null;
        return new Snapshot(generation, uri);
    }

    static final class Snapshot {
        final int generation;
        final String uri;
        Snapshot(int generation, String uri) { this.generation = generation; this.uri = uri; }
    }
}
