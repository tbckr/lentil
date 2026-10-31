package app.lentil.ui

import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.res.stringResource
import app.lentil.R
import app.lentil.data.Finding
import app.lentil.data.Intent
import app.lentil.data.LedgerError
import app.lentil.data.StoredConflict

/** A translated message and the finding arguments for its placeholders, in order. */
private class Message(@param:StringRes val id: Int, vararg val args: String)

/** Every finding code of the core, keyed by its name (lentil-ffi `finding_code_name`). */
private val findingMessages =
    mapOf(
        "ConfigInvalid" to Message(R.string.finding_config_invalid, "detail"),
        "SpecVersionUnsupported" to
            Message(R.string.finding_spec_version_unsupported, "found", "supported"),
        "ParseError" to Message(R.string.finding_parse_error, "detail"),
        "TransactionUnbalanced" to Message(R.string.finding_transaction_unbalanced, "detail"),
        "AccountNotOpen" to Message(R.string.finding_account_not_open, "account"),
        "DuplicateLentilId" to Message(R.string.finding_duplicate_lentil_id, "id"),
        "OutsideWriteArea" to Message(R.string.finding_outside_write_area, "path", "role"),
        "OutsideProfile" to Message(R.string.finding_outside_profile, "construct"),
        "MainFileMismatch" to Message(R.string.finding_main_file_mismatch, "detail"),
        "IncludeOutsideRepository" to
            Message(R.string.finding_include_outside_repository, "include"),
        "MissingZeroTolerance" to Message(R.string.finding_missing_zero_tolerance, "account"),
        "ManualPostingOnSourceAccount" to
            Message(R.string.finding_manual_posting_on_source_account, "account", "id"),
        "PadOnSourceAccount" to Message(R.string.finding_pad_on_source_account, "account"),
        "ProtectedMetadataChanged" to
            Message(R.string.finding_protected_metadata_changed, "id", "key"),
        "ProtectedTransactionChanged" to
            Message(R.string.finding_protected_transaction_changed, "id"),
        "StrayLedgerFile" to Message(R.string.finding_stray_ledger_file, "path"),
        "InvalidPath" to Message(R.string.finding_invalid_path, "path"),
        "LoadCacheCommitted" to Message(R.string.finding_load_cache_committed, "path"),
        "CoreError" to Message(R.string.finding_core_error, "code", "detail"),
        "BalanceAssertionFailed" to
            Message(
                R.string.finding_balance_assertion_failed,
                "account",
                "date",
                "difference",
                "currency",
                "asserted",
            ),
    )

/** Every conflict code of the core, keyed by its name (lentil-ffi `conflict_code_name`). */
private val conflictMessages =
    mapOf(
        "TransactionNotFound" to R.string.conflict_transaction_not_found,
        "TransactionDismissed" to R.string.conflict_transaction_dismissed,
        "AccountChanged" to R.string.conflict_account_changed,
        "PostingNotFound" to R.string.conflict_posting_not_found,
        "SourcePosting" to R.string.conflict_source_posting,
        "AccountNotOpen" to R.string.conflict_account_not_open,
        "NotEditable" to R.string.conflict_not_editable,
        "FileExists" to R.string.conflict_file_exists,
        "InvalidAccount" to R.string.conflict_invalid_account,
    )

@Composable
@Suppress("SpreadOperator") // At most five arguments.
fun findingText(finding: Finding): String {
    val message = findingMessages[finding.code]
    val text =
        if (message == null) {
            stringResource(R.string.finding_unknown, finding.code)
        } else {
            val args = message.args.map { finding.args[it] ?: "?" }.toTypedArray<Any>()
            stringResource(message.id, *args)
        }
    val location = finding.file?.let { file -> finding.line?.let { "$file:$it" } ?: file }
    return if (location == null) text else stringResource(R.string.finding_location, text, location)
}

@Composable
fun conflictText(conflict: StoredConflict): String {
    val id = conflictMessages[conflict.code]
    return when {
        id == null -> stringResource(R.string.conflict_unknown, conflict.code)
        conflict.code == "AccountChanged" ->
            stringResource(id, conflict.current ?: stringResource(R.string.unknown_value))
        else -> stringResource(id)
    }
}

@Composable
fun intentText(intent: Intent): String =
    when (intent) {
        is Intent.Confirm -> stringResource(R.string.intent_confirm)
        is Intent.SetAccount -> stringResource(R.string.intent_set_account, intent.new)
        is Intent.Dismiss -> stringResource(R.string.intent_dismiss)
        is Intent.OpenAccount -> stringResource(R.string.intent_open_account, intent.account)
    }

@Composable
fun errorText(error: LedgerError): String =
    when (error) {
        LedgerError.Network -> stringResource(R.string.error_network)
        LedgerError.Auth -> stringResource(R.string.error_auth)
        LedgerError.NotFound -> stringResource(R.string.error_not_found)
        LedgerError.LocalChanges -> stringResource(R.string.error_local_changes)
        is LedgerError.ReadOnly -> stringResource(R.string.error_read_only)
        is LedgerError.Blocked -> stringResource(R.string.error_blocked)
        LedgerError.TooManyAttempts -> stringResource(R.string.error_too_many_attempts)
        is LedgerError.Io -> stringResource(R.string.error_io, error.detail)
        is LedgerError.Other -> stringResource(R.string.error_other, error.detail)
    }
