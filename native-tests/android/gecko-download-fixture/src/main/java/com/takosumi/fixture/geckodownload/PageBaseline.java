package com.takosumi.fixture.geckodownload;

/** Initial editor/gesture admission only; navigation and admission run on the main thread.
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
        if (!editorVisible || !navigationComplete(selected, attached, generation, uri)) return null;
        return new Snapshot(generation, uri);
    }

    private boolean navigationComplete(Object selected, Object attached, int generation, String uri) {
        return !(selected != session || attached != session || generation <= 0
                || started != generation || completed != generation || uri == null || !uri.startsWith(origin));
    }

    /** Per-entry budget. A rejected navigation never consumes the one existing gesture. */
    final class Entry {
        private volatile boolean tapped;
        private volatile boolean scrolled;

        boolean tapped() { return tapped; }
        boolean scrolled() { return scrolled; }

        boolean admit(boolean tap, Object selected, Object attached, int generation, String uri) {
            if (tapped || (!tap && scrolled) || !navigationComplete(selected, attached, generation, uri)) return false;
            if (tap) tapped = true;
            else scrolled = true;
            return true;
        }
    }

    static final class Snapshot {
        final int generation;
        final String uri;
        Snapshot(int generation, String uri) { this.generation = generation; this.uri = uri; }
    }
}
