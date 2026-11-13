package com.takosumi.appscreen

import java.util.concurrent.Executor
import java.util.concurrent.RejectedExecutionException

/** UI-owned coordination; the worker receives only a captured cookie-jar operation.
 * At most one operation per profile runs, with one coalesced later request.
 * Clear drains an already running flush and discards redundant/stale requests.
 */
internal class AppScreenCookiePersistence(
    private val worker: Executor,
    private val post: (() -> Unit) -> Unit,
    private val checkUi: () -> Unit,
    private val generation: (String) -> Long,
    private val receipt: (Owner, Outcome) -> Unit,
) {
    class Owner(val profile: String, val generation: Long, val flush: () -> Unit)
    enum class Outcome { FLUSHED, FAILED, RETIRED, BUSY }
    class Clear internal constructor(internal val state: State)
    internal class State(val owner: Owner) {
        var running = false
        var later: Owner? = null
        var clear: Clear? = null
        var drained: ((Clear) -> Unit)? = null
    }

    private val states = mutableMapOf<String, State>()

    fun request(owner: Owner): Boolean {
        checkUi()
        if (owner.generation < 0) {
            receipt(owner, Outcome.FAILED)
            return false
        }
        if (generation(owner.profile) != owner.generation) {
            receipt(owner, Outcome.RETIRED)
            return false
        }
        val existing = states[owner.profile]
        if (existing != null) {
            if (existing.clear != null || existing.owner.generation != owner.generation) {
                receipt(owner, Outcome.RETIRED)
                return false
            }
            existing.later = owner
            return true
        }
        if (states.size >= MAX_PROFILES) {
            receipt(owner, Outcome.BUSY)
            return false
        }
        val state = State(owner)
        states[owner.profile] = state
        return start(state, owner)
    }

    /** Reserve before starting deletion. No new flush can cross this barrier. */
    fun beginClear(profile: String, atGeneration: Long, drained: (Clear) -> Unit): Boolean {
        checkUi()
        if (atGeneration < 0 || generation(profile) != atGeneration) return false
        val previous = states[profile]
        if (previous?.clear != null || (previous != null && previous.owner.generation != atGeneration)) return false
        if (previous == null && states.size >= MAX_PROFILES) return false
        val state = previous ?: State(Owner(profile, atGeneration) {}).also { states[profile] = it }
        val clear = Clear(state)
        state.clear = clear
        state.later = null
        state.drained = drained
        if (!state.running) deliverClear(state)
        return true
    }

    /** Called only after deletion and the existing generation/preferences commit. */
    fun finishClear(clear: Clear) {
        checkUi()
        val state = clear.state
        if (states[state.owner.profile] === state && state.clear === clear && !state.running) {
            states.remove(state.owner.profile)
            state.drained = null
        }
    }

    private fun start(state: State, owner: Owner): Boolean {
        state.running = true
        try {
            worker.execute {
                val success = try {
                    owner.flush()
                    true
                } catch (_: InterruptedException) {
                    Thread.currentThread().interrupt()
                    false
                } catch (_: Exception) {
                    false
                }
                post { completed(state, owner, success) }
            }
            return true
        } catch (_: RejectedExecutionException) {
            completed(state, owner, success = false)
            return false
        }
    }

    private fun completed(state: State, owner: Owner, success: Boolean) {
        checkUi()
        if (states[owner.profile] !== state || !state.running) return
        state.running = false
        val current = state.clear == null && generation(owner.profile) == owner.generation
        receipt(owner, when {
            !success -> Outcome.FAILED
            !current -> Outcome.RETIRED
            else -> Outcome.FLUSHED
        })
        if (state.clear != null) {
            deliverClear(state)
            return
        }
        val later = state.later
        state.later = null
        if (current && later != null) start(state, later)
        else states.remove(owner.profile)
    }

    private fun deliverClear(state: State) {
        val callback = state.drained ?: return
        state.drained = null
        callback(requireNotNull(state.clear))
    }

    companion object {
        const val MAX_PROFILES = 64
    }
}
