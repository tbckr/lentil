@file:OptIn(ExperimentalMaterial3Api::class)

package app.lentil.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import app.lentil.R
import app.lentil.data.LedgerAccount
import app.lentil.data.usableOn

/**
 * Picks a counter account for a transaction on [date]: search, recently used first, or a new
 * account (PRD: FR-D3). It offers only accounts open on [date], because changing the account also
 * confirms the transaction, and a change the core rejects would confirm it on the old account.
 */
@Composable
fun AccountSheet(
    accounts: List<LedgerAccount>,
    date: String,
    recent: List<String>,
    onSelect: (String) -> Unit,
    onOpen: (String) -> Unit,
    onClose: () -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val trimmed = query.trim()
    val usable = accounts.usableOn(date)
    val recentOpen = recent.filter { it in usable }
    val matches = usable.filter { it.contains(trimmed, ignoreCase = true) }
    ModalBottomSheet(onDismissRequest = onClose) {
        OutlinedTextField(
            value = query,
            onValueChange = { query = it },
            label = { Text(stringResource(R.string.account_search)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        )
        LazyColumn(modifier = Modifier.fillMaxWidth()) {
            if (trimmed.isEmpty() && recentOpen.isNotEmpty()) {
                item { Header(stringResource(R.string.account_recent)) }
                items(recentOpen, key = { "recent:$it" }) { Account(it, onSelect) }
                item { Header(stringResource(R.string.account_all)) }
            }
            if (isAccountName(trimmed) && accounts.none { it.name == trimmed }) {
                item {
                    ListItem(
                        colors = transparent(),
                        headlineContent = {
                            Text(stringResource(R.string.account_open_new, trimmed))
                        },
                        modifier = Modifier.clickable { onOpen(trimmed) },
                    )
                }
            }
            items(matches, key = { "all:$it" }) { Account(it, onSelect) }
        }
    }
}

@Composable
private fun Header(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelLarge,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
    )
}

@Composable
private fun Account(account: String, onSelect: (String) -> Unit) {
    ListItem(
        colors = transparent(),
        headlineContent = { Text(account) },
        modifier = Modifier.clickable { onSelect(account) },
    )
}

/** List items on the sheet's own background. */
@Composable private fun transparent() = ListItemDefaults.colors(containerColor = Color.Transparent)

/**
 * Whether [name] looks like a Beancount account, such as `Expenses:Garden`. Only a hint for the
 * "open" entry; the core decides and reports an invalid name as a conflict.
 */
internal fun isAccountName(name: String): Boolean = ACCOUNT.matches(name)

private val ACCOUNT =
    Regex("""(Assets|Liabilities|Equity|Income|Expenses)(:[\p{Lu}\p{N}][\p{L}\p{N}-]*)+""")
