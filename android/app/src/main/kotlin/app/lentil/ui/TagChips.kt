package app.lentil.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SuggestionChip
import androidx.compose.material3.SuggestionChipDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import app.lentil.R
import app.lentil.data.QueueEntry

/**
 * The tags of [entry], the suggested trip tag and "Add tag" (PRD: FR-D12, FR-D13). None of them
 * confirms the transaction. A transaction without ID shows its tags only.
 */
@Composable
internal fun TagChips(
    entry: QueueEntry,
    editable: Boolean,
    onAddTag: () -> Unit,
    onRemoveTag: (String) -> Unit,
    onAcceptSuggestion: () -> Unit,
) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        entry.tags.forEach { tag ->
            TagChip(
                tag = tag,
                removable = entry.editable,
                enabled = editable,
                onRemove = { onRemoveTag(tag) },
            )
        }
        if (entry.editable) {
            entry.suggestedTag?.let { tag ->
                SuggestionChip(
                    onClick = onAcceptSuggestion,
                    enabled = editable,
                    label = { Text("#$tag") },
                    icon = {
                        Icon(
                            painterResource(R.drawable.ic_add),
                            contentDescription = stringResource(R.string.tag_suggested, tag),
                            modifier = Modifier.size(SuggestionChipDefaults.IconSize),
                        )
                    },
                )
            }
            AssistChip(
                onClick = onAddTag,
                enabled = editable,
                label = { Text(stringResource(R.string.tag_add)) },
                leadingIcon = {
                    Icon(
                        painterResource(R.drawable.ic_add),
                        contentDescription = null,
                        modifier = Modifier.size(AssistChipDefaults.IconSize),
                    )
                },
            )
        }
    }
}

/**
 * A tag of a transaction. Only its × removes it, as a button of its own, so a stray tap on the
 * label never removes a tag that the picker might not offer again.
 */
@Composable
private fun TagChip(tag: String, removable: Boolean, enabled: Boolean, onRemove: () -> Unit) {
    Box(modifier = Modifier.heightIn(min = 48.dp), contentAlignment = Alignment.Center) {
        Surface(
            shape = MaterialTheme.shapes.small,
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        ) {
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier =
                    Modifier.height(32.dp)
                        .padding(start = 12.dp, end = if (removable) 4.dp else 12.dp),
            ) {
                Text("#$tag", style = MaterialTheme.typography.labelLarge)
                if (removable) {
                    Icon(
                        painterResource(R.drawable.ic_close),
                        contentDescription = stringResource(R.string.tag_remove, tag),
                        tint =
                            if (enabled) {
                                MaterialTheme.colorScheme.onSurfaceVariant
                            } else {
                                MaterialTheme.colorScheme.onSurface.copy(alpha = DISABLED_ALPHA)
                            },
                        modifier =
                            Modifier.padding(start = 4.dp)
                                .clip(CircleShape)
                                .clickable(
                                    enabled = enabled,
                                    role = Role.Button,
                                    onClick = onRemove,
                                )
                                .padding(3.dp)
                                .size(18.dp),
                    )
                }
            }
        }
    }
}

/** Material's opacity for disabled content. */
private const val DISABLED_ALPHA = 0.38f
