<script lang="ts">
	import * as AlertDialog from '$lib/components/ui/alert-dialog';

	let {
		open = $bindable(),
		title = 'Are you absolutely sure?',
		description = 'This action cannot be undone.',
		itemName,
		onConfirm
	} = $props();
</script>

<AlertDialog.Root bind:open>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{title}</AlertDialog.Title>
			<AlertDialog.Description>
				{description}
				{#if itemName}
					This will permanently delete <strong>{itemName}</strong>.
				{/if}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				onclick={onConfirm}
				class="bg-destructive text-destructive-foreground hover:bg-destructive/90"
			>
				Delete
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
