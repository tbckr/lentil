package app.lentil.data

import app.lentil.ffi.FfiAuthor
import app.lentil.ffi.FfiCredentials
import app.lentil.ffi.FfiFinding
import app.lentil.ffi.FfiIdentity
import app.lentil.ffi.FfiIntent
import app.lentil.ffi.FfiOutcome
import app.lentil.ffi.FfiPendingIntent
import app.lentil.ffi.FfiSeverity
import app.lentil.ffi.LentilException
import app.lentil.ffi.LentilRepo
import app.lentil.ffi.QueueItem
import app.lentil.ffi.newDeviceId as ffiNewDeviceId
import app.lentil.ffi.useCaBundle
import java.io.File
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * [LedgerRepository] on the Rust core: a bare clone in [directory], driven through the generated
 * UniFFI bindings. Calls block, so they run on [io].
 */
class FfiLedgerRepository(
    private val directory: File,
    private val caBundle: SystemCaBundle,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) : LedgerRepository {
    private var opened: LentilRepo? = null

    override fun hasClone(): Boolean = directory.isDirectory

    override suspend fun clone(url: String, credentials: Credentials) = call {
        prepareNetwork()
        directory.deleteRecursively()
        opened = LentilRepo.cloneRepo(url, directory.path, credentials.toFfi())
    }

    override suspend fun load(pending: List<StoredIntent>): LedgerView = call {
        val repo = repo()
        val intents = pending.map { it.toFfi() }
        val readOnly = repo.readOnlyReasons().map { it.toFinding() }
        LedgerView(
            queue = repo.queue(intents).map { it.toEntry() },
            accounts = repo.accounts(intents),
            defaultOpenDate = repo.defaultOpenDate(intents),
            readOnly = readOnly,
        )
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
