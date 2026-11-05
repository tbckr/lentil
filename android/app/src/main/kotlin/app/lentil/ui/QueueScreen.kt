@file:OptIn(ExperimentalMaterial3Api::class)

package app.lentil.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import app.lentil.R
import app.lentil.data.Finding
import app.lentil.data.QueueEntry
import app.lentil.data.StoredConflict
import java.text.NumberFormat
import java.util.Locale

@Composable
fun QueueScreen(viewModel: QueueViewModel, onChangeConnection: () -> Unit) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    var accountFor by remember { mutableStateOf<QueueEntry?>(null) }
    var dismissFor by remember { mutableStateOf<QueueEntry?>(null) }
    var startingTrip by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.queue_title)) },
                actions = {
                    IconButton(onClick = viewModel::sync) {
                        Icon(
                            painterResource(R.drawable.ic_sync),
                            contentDescription = stringResource(R.string.action_sync),
                        )
                    }
                    QueueMenu(
                        runningTrip = state.runningTrip,
                        editable = state.tripsEditable,
                        onStartTrip = { startingTrip = true },
                        onEndTrip = viewModel::endTrip,
                        onChangeConnection = onChangeConnection,
                    )
                },
            )
        }
    ) { padding ->
        PullToRefreshBox(
            isRefreshing = state.sync == SyncStatus.Syncing,
            onRefresh = viewModel::sync,
            modifier = Modifier.fillMaxSize().padding(padding),
        ) {
            LazyColumn(
                contentPadding = PaddingValues(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                item { StatusLine(state) }
                if (state.readOnly.isNotEmpty()) {
                    item { FindingsCard(stringResource(R.string.read_only_title), state.readOnly) }
                }
                (state.sync as? SyncStatus.Blocked)?.let { blocked ->
                    item { FindingsCard(stringResource(R.string.blocked_title), blocked.findings) }
                }
                if (state.warnings.isNotEmpty()) {
                    item { FindingsCard(stringResource(R.string.warnings_title), state.warnings) }
                }
                if (state.conflicts.isNotEmpty()) {
                    item { ConflictsCard(state.conflicts, viewModel::discardConflict) }
                }
                state.loadError?.let { error ->
                    item { Text(errorText(error), color = MaterialTheme.colorScheme.error) }
                }
                if (state.items.isEmpty()) {
                    item {
                        Text(
                            stringResource(
                                if (state.loading) R.string.queue_loading else R.string.queue_empty
                            ),
                            style = MaterialTheme.typography.bodyLarge,
                        )
                    }
                }
                items(state.items, key = { it.id ?: "${it.file}:${it.line}" }) { entry ->
                    QueueRow(
                        entry = entry,
                        writable = state.writable,
                        onConfirm = { viewModel.confirm(entry) },
                        onAccount = { accountFor = entry },
                        onDismiss = { dismissFor = entry },
                        onAddTag = { viewModel.openTagPicker(entry) },
                        onRemoveTag = { tag -> viewModel.removeTag(entry, tag) },
                        onAcceptSuggestion = { viewModel.acceptSuggestion(entry) },
                    )
                }
            }
        }
    }

    accountFor?.let { entry ->
        AccountSheet(
            accounts = state.accounts,
            date = entry.date,
            recent = state.recentAccounts,
            onSelect = { account ->
                viewModel.changeAccount(entry, account)
                accountFor = null
            },
            onOpen = { account ->
                viewModel.openAccount(entry, account)
                accountFor = null
            },
            onClose = { accountFor = null },
        )
    }

    state.tagPicker?.let { picker ->
        TagSheet(
            choices = picker.choices,
            isPlainName = viewModel::isPlainName,
            onPick = { tag -> viewModel.pickTag(picker.entry, tag) },
            onClose = viewModel::closeTagPicker,
        )
    }

    if (startingTrip) {
        StartTripDialog(
            isPlainName = viewModel::isPlainName,
            onStart = { name ->
                viewModel.startTrip(name)
                startingTrip = false
            },
            onClose = { startingTrip = false },
        )
    }

    dismissFor?.let { entry ->
        AlertDialog(
            onDismissRequest = { dismissFor = null },
            title = { Text(stringResource(R.string.dismiss_title)) },
            text = { Text(stringResource(R.string.dismiss_text)) },
            confirmButton = {
                TextButton(
                    onClick = {
                        viewModel.dismiss(entry)
                        dismissFor = null
                    }
                ) {
                    Text(stringResource(R.string.dismiss_confirm))
                }
            },
            dismissButton = {
                TextButton(onClick = { dismissFor = null }) {
                    Text(stringResource(R.string.cancel))
                }
            },
        )
    }
}

/** The top bar's menu: start or end a trip (PRD: FR-D13), and change the connection. */
@Composable
private fun QueueMenu(
    runningTrip: String?,
    editable: Boolean,
    onStartTrip: () -> Unit,
    onEndTrip: () -> Unit,
    onChangeConnection: () -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { menu = true }) {
            Icon(
                painterResource(R.drawable.ic_more_vert),
                contentDescription = stringResource(R.string.action_more),
            )
        }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
            DropdownMenuItem(
                text = {
                    Text(
                        if (runningTrip == null) {
                            stringResource(R.string.action_start_trip)
                        } else {
                            stringResource(R.string.action_end_trip, runningTrip)
                        }
                    )
                },
                enabled = editable,
                onClick = {
                    menu = false
                    if (runningTrip == null) onStartTrip() else onEndTrip()
                },
            )
            DropdownMenuItem(
                text = { Text(stringResource(R.string.action_change_connection)) },
                onClick = {
                    menu = false
                    onChangeConnection()
                },
            )
        }
    }
}

@Composable
private fun StatusLine(state: QueueUiState) {
    val problem =
        when (val sync = state.sync) {
            SyncStatus.Idle,
            SyncStatus.UpToDate -> null
            SyncStatus.Syncing -> stringResource(R.string.status_syncing)
            SyncStatus.Offline -> stringResource(R.string.status_offline)
            SyncStatus.SignInFailed -> stringResource(R.string.status_sign_in_failed)
            SyncStatus.RemoteMissing -> stringResource(R.string.status_remote_missing)
            SyncStatus.Busy -> stringResource(R.string.status_busy)
            is SyncStatus.Blocked -> stringResource(R.string.status_blocked)
            is SyncStatus.ReadOnly -> stringResource(R.string.status_read_only)
            is SyncStatus.Failed -> stringResource(R.string.status_failed, sync.detail)
        }
    val pending =
        if (state.pendingCount > 0) {
            pluralStringResource(R.plurals.pending_changes, state.pendingCount, state.pendingCount)
        } else {
            null
        }
    val upToDate =
        stringResource(R.string.status_up_to_date).takeIf {
            state.sync == SyncStatus.UpToDate && pending == null
        }
    val trip = state.runningTrip?.let { stringResource(R.string.status_trip, it) }
    Column {
        listOfNotNull(trip, upToDate, pending, problem).forEach {
            Text(it, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun QueueRow(
    entry: QueueEntry,
    writable: Boolean,
    onConfirm: () -> Unit,
    onAccount: () -> Unit,
    onDismiss: () -> Unit,
    onAddTag: () -> Unit,
    onRemoveTag: (String) -> Unit,
    onAcceptSuggestion: () -> Unit,
) {
    val editable = writable && entry.editable
    var menu by remember { mutableStateOf(false) }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.Top) {
                Column(modifier = Modifier.weight(1f)) {
                    Text(
                        entry.payee ?: entry.narration,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (entry.payee != null && entry.narration.isNotBlank()) {
                        Text(entry.narration, style = MaterialTheme.typography.bodyMedium)
                    }
                    entry.desc?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = FontFamily.Monospace,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    Text(entry.date, style = MaterialTheme.typography.bodySmall)
                }
                Spacer(Modifier.width(12.dp))
                entry.amount?.let {
                    Text(
                        formatAmount(it, entry.currency, LocalConfiguration.current.locales[0]),
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(modifier = Modifier.weight(1f)) {
                    val counter = entry.counterAccount
                    when {
                        !entry.editable -> Hint(stringResource(R.string.no_id_read_only))
                        counter == null -> Hint(stringResource(R.string.split_read_only))
                        else ->
                            AssistChip(
                                onClick = onAccount,
                                enabled = editable,
                                label = {
                                    Text(counter, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                },
                            )
                    }
                }
                IconButton(onClick = onConfirm, enabled = editable) {
                    Icon(
                        painterResource(R.drawable.ic_check),
                        contentDescription = stringResource(R.string.action_confirm),
                    )
                }
                Box {
                    IconButton(onClick = { menu = true }, enabled = editable) {
                        Icon(
                            painterResource(R.drawable.ic_more_vert),
                            contentDescription = stringResource(R.string.action_more),
                        )
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text(stringResource(R.string.action_dismiss)) },
                            onClick = {
                                menu = false
                                onDismiss()
                            },
                        )
                    }
                }
            }
            TagChips(entry, editable, onAddTag, onRemoveTag, onAcceptSuggestion)
        }
    }
}

/** Asks for the name of a new trip; only a name by the core's rule can start it. */
@Composable
private fun StartTripDialog(
    isPlainName: (String) -> Boolean,
    onStart: (String) -> Unit,
    onClose: () -> Unit,
) {
    var name by remember { mutableStateOf("") }
    val trimmed = name.trim()
    AlertDialog(
        onDismissRequest = onClose,
        title = { Text(stringResource(R.string.trip_start_title)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.trip_start_text))
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    label = { Text(stringResource(R.string.trip_name)) },
                    supportingText = { Text(stringResource(R.string.trip_name_rule)) },
                    isError = breaksNameRule(trimmed, isPlainName),
                    singleLine = true,
                    keyboardOptions = NAME_KEYBOARD,
                    keyboardActions =
                        KeyboardActions(onDone = { if (isPlainName(trimmed)) onStart(trimmed) }),
                )
            }
        },
        confirmButton = {
            TextButton(onClick = { onStart(trimmed) }, enabled = isPlainName(trimmed)) {
                Text(stringResource(R.string.trip_start_confirm))
            }
        },
        dismissButton = {
            TextButton(onClick = onClose) { Text(stringResource(R.string.cancel)) }
        },
    )
}

@Composable
private fun Hint(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

@Composable
private fun FindingsCard(title: String, findings: List<Finding>) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(title, style = MaterialTheme.typography.titleSmall)
            findings.forEach { Text(findingText(it), style = MaterialTheme.typography.bodySmall) }
        }
    }
}

@Composable
private fun ConflictsCard(conflicts: List<StoredConflict>, onDiscard: (String) -> Unit) {
    Card(
        colors =
            CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.tertiaryContainer),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                pluralStringResource(R.plurals.conflicts_title, conflicts.size, conflicts.size),
                style = MaterialTheme.typography.titleSmall,
            )
            conflicts.forEach { conflict ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(modifier = Modifier.weight(1f).padding(vertical = 4.dp)) {
                        Text(conflict.intent.label, style = MaterialTheme.typography.bodyMedium)
                        Text(
                            "${intentText(conflict.intent.intent)}: ${conflictText(conflict)}",
                            style = MaterialTheme.typography.bodySmall,
                            color = Color.Unspecified,
                        )
                    }
                    TextButton(onClick = { onDiscard(conflict.intent.key) }) {
                        Text(stringResource(R.string.conflict_discard))
                    }
                }
            }
        }
    }
}

/** Formats a decimal from the ledger for [locale], keeping its number of decimal places. */
internal fun formatAmount(amount: String, currency: String?, locale: Locale): String {
    val number =
        amount.toBigDecimalOrNull() ?: return listOfNotNull(amount, currency).joinToString(" ")
    val format =
        NumberFormat.getNumberInstance(locale).apply {
            minimumFractionDigits = number.scale().coerceAtLeast(0)
            maximumFractionDigits = number.scale().coerceAtLeast(0)
        }
    return listOfNotNull(format.format(number), currency).joinToString(" ")
}
