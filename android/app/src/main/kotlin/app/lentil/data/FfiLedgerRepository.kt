package app.lentil.data

import app.lentil.ffi.FfiAuthor
import app.lentil.ffi.FfiChoiceSource
import app.lentil.ffi.FfiCredentials
import app.lentil.ffi.FfiFinding
import app.lentil.ffi.FfiIdentity
import app.lentil.ffi.FfiIntent
import app.lentil.ffi.FfiOutcome
import app.lentil.ffi.FfiPendingIntent
import app.lentil.ffi.FfiSeverity
import app.lentil.ffi.FfiTagChoices
import app.lentil.ffi.LentilException
import app.lentil.ffi.LentilRepo
import app.lentil.ffi.QueueItem
import app.lentil.ffi.isPlainName as ffiIsPlainName
import app.lentil.ffi.newDeviceId as ffiNewDeviceId
import app.lentil.ffi.newManualId as ffiNewManualId
import app.lentil.ffi.useCaBundle
import java.io.File
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * [LedgerRepository] on the Rust core: a bare clone in [directory], driven through the generated
 * UniFFI bindings. Calls block, so they run on [io].
 */
@Suppress("TooManyFunctions") // One function per call of the repository, plus two helpers.
class FfiLedgerRepository(
    private val directory: File,
    private val caBundle: SystemCaBundle,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) : LedgerRepository {
    private var opened: LentilRepo? = null

    override fun hasClone(): Boolean = directory.isDirectory

    /** Clones next to the current clone and replaces it only on success. */
    override suspend fun clone(url: String, credentials: Credentials) = call {
        prepareNetwork()
        val fresh = File(directory.parentFile, "${directory.name}.new")
        fresh.deleteRecursively()
        LentilRepo.cloneRepo(url, fresh.path, credentials.toFfi()).destroy()
        opened?.destroy()
        opened = null
        directory.deleteRecursively()
        check(fresh.renameTo(directory)) { "cannot replace ${directory.name}" }
    }

    override suspend fun load(pending: List<StoredIntent>): LedgerView = call {
        val repo = repo()
        val intents = pending.map { it.toFfi() }
        val readOnly = repo.readOnlyReasons().map { it.toFinding() }
        LedgerView(
            queue = repo.queue(intents).map { it.toEntry() },
            accounts = repo.accounts(intents).map { LedgerAccount(it.name, it.openFrom) },
            defaultOpenDate = repo.defaultOpenDate(intents),
            readOnly = readOnly,
            runningTrip = repo.runningTrip(intents),
        )
    }

    override suspend fun tagChoices(pending: List<StoredIntent>, id: String): TagChoices = call {
        repo().tagChoices(pending.map { it.toFfi() }, id).toChoices()
    }

    override suspend fun pickTag(
        pending: List<StoredIntent>,
        id: String,
        tag: String,
    ): List<Intent> = call {
        repo().pickTag(pending.map { it.toFfi() }, id, tag).map { it.toIntent() }
    }

    override suspend fun sync(
        pending: List<StoredIntent>,
        identity: Identity,
        author: Author,
        credentials: Credentials,
    ): SyncResult = call {
        prepareNetwork()
        val report =
            repo()
                .sync(
                    pending.map { it.toFfi() },
                    FfiIdentity(client = identity.client, device = identity.device),
                    FfiAuthor(name = author.name, email = author.email),
                    credentials.toFfi(),
                )
        SyncResult(
            outcomes = report.outcomes.associate { it.key to it.outcome.toOutcome() },
            committed = report.committed,
            warnings = report.warnings.map { it.toFinding() },
        )
    }

    override fun newDeviceId(): String = ffiNewDeviceId()

    override fun newManualId(device: String): String = ffiNewManualId(device)

    override fun isPlainName(name: String): Boolean = ffiIsPlainName(name)

    private fun repo(): LentilRepo = opened ?: LentilRepo.open(directory.path).also { opened = it }

    /** Hands the system's trusted CAs to the core once per process, before any network use. */
    private fun prepareNetwork() {
        if (!caBundlePrepared) {
            useCaBundle(caBundle.export().path)
            caBundlePrepared = true
        }
    }

    /** Runs [block] on [io] and turns every failure into a [LedgerException]. */
    @Suppress("TooGenericExceptionCaught")
    private suspend fun <T> call(block: () -> T): T =
        withContext(io) {
            try {
                block()
            } catch (e: LentilException) {
                throw LedgerException(e.toError(), e)
            } catch (e: Exception) {
                // A bug in the core arrives as UniFFI's InternalException; show it, don't crash.
                throw LedgerException(LedgerError.Other(e.message ?: e.javaClass.simpleName), e)
            }
        }

    private companion object {
        @Volatile var caBundlePrepared = false
    }
}

private fun Credentials.toFfi(): FfiCredentials =
    FfiCredentials.UserPassword(username = username, password = token)

private fun StoredIntent.toFfi(): FfiPendingIntent =
    FfiPendingIntent(
        key = key,
        intent =
            when (val intent = intent) {
                is Intent.Confirm -> FfiIntent.Confirm(intent.id)
                is Intent.SetAccount ->
                    FfiIntent.SetAccount(
                        id = intent.id,
                        posting = intent.posting.toUInt(),
                        old = intent.old,
                        new = intent.new,
                    )
                is Intent.Dismiss -> FfiIntent.Dismiss(intent.id)
                is Intent.OpenAccount -> FfiIntent.OpenAccount(intent.account, intent.date)
                is Intent.AddTag -> FfiIntent.AddTag(intent.id, intent.tag)
                is Intent.RemoveTag -> FfiIntent.RemoveTag(intent.id, intent.tag)
                is Intent.StartTrip -> FfiIntent.StartTrip(intent.id, intent.date, intent.name)
                is Intent.EndTrip -> FfiIntent.EndTrip(intent.id, intent.date)
            },
    )

private fun FfiIntent.toIntent(): Intent =
    when (this) {
        is FfiIntent.Confirm -> Intent.Confirm(id)
        is FfiIntent.SetAccount ->
            Intent.SetAccount(id = id, posting = posting.toInt(), old = old, new = new)
        is FfiIntent.Dismiss -> Intent.Dismiss(id)
        is FfiIntent.OpenAccount -> Intent.OpenAccount(account, date)
        is FfiIntent.AddTag -> Intent.AddTag(id, tag)
        is FfiIntent.RemoveTag -> Intent.RemoveTag(id, tag)
        is FfiIntent.StartTrip -> Intent.StartTrip(id, date, name)
        is FfiIntent.EndTrip -> Intent.EndTrip(id, date)
    }

private fun FfiTagChoices.toChoices(): TagChoices =
    TagChoices(
        tags = tags.map { TagChoice(it.tag, it.replaces) },
        families = families,
        source =
            when (source) {
                FfiChoiceSource.TAXONOMY -> TagSource.Taxonomy
                FfiChoiceSource.MISSING -> TagSource.Missing
                FfiChoiceSource.INVALID -> TagSource.Invalid
            },
    )

private fun QueueItem.toEntry(): QueueEntry =
    QueueEntry(
        id = id,
        date = date,
        payee = payee,
        narration = narration,
        desc = desc,
        amount = amount,
        currency = currency,
        sourceAccount = sourceAccount,
        counterAccount = counterAccount,
        counterPosting = counterPosting?.toInt(),
        file = file,
        line = line.toInt(),
        tags = tags,
        suggestedTag = suggestedTag,
    )

private fun FfiFinding.toFinding(): Finding =
    Finding(
        code = code,
        blocking = severity == FfiSeverity.BLOCKING,
        file = file,
        line = line?.toInt(),
        args = args,
    )

private fun FfiOutcome.toOutcome(): IntentOutcome =
    when (this) {
        is FfiOutcome.Applied -> IntentOutcome.Applied
        is FfiOutcome.AlreadyApplied -> IntentOutcome.AlreadyApplied
        is FfiOutcome.Conflict -> IntentOutcome.Conflict(code, current)
    }

private fun LentilException.toError(): LedgerError =
    when (this) {
        is LentilException.Network -> LedgerError.Network
        is LentilException.Auth -> LedgerError.Auth
        is LentilException.NotFound -> LedgerError.NotFound
        is LentilException.LocalChanges -> LedgerError.LocalChanges
        is LentilException.ReadOnly -> LedgerError.ReadOnly(findings.map { it.toFinding() })
        is LentilException.Blocked -> LedgerError.Blocked(findings.map { it.toFinding() })
        is LentilException.TooManyAttempts -> LedgerError.TooManyAttempts
        is LentilException.Io -> LedgerError.Io(detail)
        is LentilException.Other -> LedgerError.Other(detail)
    }
