package com.takosumi.appscreen

/** Native document outcome, separate from URL eligibility. Shared transition
 * fixtures also exercise the Swift and desktop Rust implementations. */
internal class AppScreenResumeState {
    var current = 0L
        private set
    private var pending = false
    private var successful = false
    private var previousSuccessful = false
    private var committed = false
    private var blocked: String? = null

    fun begin(id: Long) {
        if (pending && current == id) return
        current = id
        previousSuccessful = successful
        pending = true
        committed = false
    }

    fun commit(id: Long) {
        if (id != current || !pending) return
        committed = true
        successful = false
    }

    fun finish(id: Long, success: Boolean, url: String?) {
        if (id != current || !pending) return
        pending = false
        if (success) {
            successful = true
            blocked = null
        } else {
            successful = !committed && previousSuccessful && url != null
            blocked = url
        }
    }

    fun canRemember(url: String): Boolean = !pending && successful && url != blocked

    fun isPending(): Boolean = pending

    fun invalidate() {
        pending = false
        successful = false
    }
}

/** WebView error callbacks carry URLs, not navigation IDs. Keep their binding
 * across attached/detached clients; an unknown failure never qualifies a page. */
internal class AppScreenResumeTracker {
    private val state = AppScreenResumeState()
    private val generations = LinkedHashMap<String, Long>()

    private fun bind(url: String?, id: Long) {
        if (url == null) return
        generations.remove(url)
        generations[url] = id
        while (generations.size > 64) generations.remove(generations.keys.first())
    }

    fun started(url: String?) {
        state.begin(state.current + 1)
        bind(url, state.current)
    }

    fun committed(url: String?) {
        if (!state.isPending()) return
        // The caller requires this to be the current view's visible commit.
        // A redirect back to an older URL need not have an onPageStarted.
        bind(url, state.current)
        state.commit(state.current)
    }

    fun finished(url: String?) {
        val id = url?.let { generations[it] }
        if (id != null && id != state.current) return
        state.finish(state.current, true, url)
    }

    fun failed(url: String?) {
        val id = url?.let { generations[it] }
        when {
            id == state.current -> state.finish(id, false, url)
            id == null -> state.invalidate() // Unknown redirect/TLS failure: keep stored metadata.
            // A known older navigation cannot complete the active one.
        }
    }

    fun canRemember(url: String): Boolean = state.canRemember(url)
}
