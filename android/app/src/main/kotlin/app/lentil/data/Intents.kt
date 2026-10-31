package app.lentil.data

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

/** An edit operation of spec section 10, recorded on the device until a sync pushes it. */
@Serializable
sealed interface Intent {
    /** Set the flag of transaction [id] to `*`. */
    @Serializable @SerialName("confirm") data class Confirm(val id: String) : Intent

    /** Change the account of posting [posting] of transaction [id] from [old] to [new]. */
    @Serializable
    @SerialName("set-account")
    data class SetAccount(val id: String, val posting: Int, val old: String, val new: String) :
        Intent

    /** Comment out transaction [id] in place, for example a duplicate. */
    @Serializable @SerialName("dismiss") data class Dismiss(val id: String) : Intent

    /** Open [account] on [date] (`YYYY-MM-DD`). */
    @Serializable
    @SerialName("open-account")
    data class OpenAccount(val account: String, val date: String) : Intent
}

/**
 * A pending intent. [key] identifies it in sync outcomes; [label] describes the transaction as the
 * user saw it, so a conflict can be shown after the transaction changed.
 */
@Serializable data class StoredIntent(val key: String, val intent: Intent, val label: String)

/** An intent that could not be applied, kept until the user discards it (PRD: FR-F4). */
@Serializable
data class StoredConflict(val intent: StoredIntent, val code: String, val current: String? = null)

/** Everything the app keeps besides the clone: pending intents and conflicts. */
@Serializable
data class IntentState(
    val pending: List<StoredIntent> = emptyList(),
    val conflicts: List<StoredConflict> = emptyList(),
) {
    fun add(intents: List<StoredIntent>): IntentState = copy(pending = pending + intents)

    fun discard(key: String): IntentState =
        copy(conflicts = conflicts.filterNot { it.intent.key == key })

    /**
     * Applies sync outcomes: applied intents leave the queue, conflicts move to the conflict list.
     * Intents without an outcome, such as those added during the sync, stay pending.
     */
    fun afterSync(outcomes: Map<String, IntentOutcome>): IntentState {
        val stillPending = mutableListOf<StoredIntent>()
        val newConflicts = mutableListOf<StoredConflict>()
        for (intent in pending) {
            when (val outcome = outcomes[intent.key]) {
                null -> stillPending += intent
                IntentOutcome.Applied,
                IntentOutcome.AlreadyApplied -> Unit
                is IntentOutcome.Conflict ->
                    newConflicts += StoredConflict(intent, outcome.code, outcome.current)
            }
        }
        return IntentState(pending = stillPending, conflicts = conflicts + newConflicts)
    }
}

/** Where pending intents live: the app's own storage, never the repository (spec section 10). */
interface IntentStore {
    suspend fun read(): IntentState

    suspend fun write(state: IntentState)
}
