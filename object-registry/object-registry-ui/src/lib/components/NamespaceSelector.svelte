<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Select from '$lib/components/ui/select';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import { Input } from '$lib/components/ui/input';
	import { Plus } from '@lucide/svelte';
	import { toast } from 'svelte-sonner';

	let {
		namespaces = [],
		namespace = $bindable(),
		onNamespaceAdded
	} = $props();

	let createNamespaceDialogOpen = $state(false);
	let newNamespaceName = $state('');

	function handleCreateNamespace() {
		if (!newNamespaceName.trim()) {
			toast.warning('Namespace name cannot be empty');
			return;
		}
		if (namespaces.includes(newNamespaceName.trim())) {
			toast.warning('Namespace already exists');
			return;
		}
		const name = newNamespaceName.trim();
		onNamespaceAdded(name);
		namespace = name;
		newNamespaceName = '';
		createNamespaceDialogOpen = false;
		toast.success(`Namespace '${name}' added to local view`);
	}

	let selectedNamespaceLabel = $derived(namespaces.find((ns) => ns === namespace) || namespace);
</script>

<Card.Root>
	<Card.Header>
		<Card.Title>Namespace Selection</Card.Title>
		<Card.Description>Select the namespace you want to browse.</Card.Description>
	</Card.Header>
	<Card.Content>
		<div class="grid w-full max-w-sm items-center gap-1.5">
			<Label for="namespace-select">Namespace</Label>
			<div class="flex gap-2">
				<Select.Root type="single" bind:value={namespace}>
					<Select.Trigger class="w-full">
						{selectedNamespaceLabel}
					</Select.Trigger>
					<Select.Content>
						{#each namespaces as ns}
							<Select.Item value={ns}>{ns}</Select.Item>
						{/each}
					</Select.Content>
				</Select.Root>
				<Button
					variant="outline"
					size="icon"
					onclick={() => (createNamespaceDialogOpen = true)}
					title="Create new namespace"
				>
					<Plus class="h-4 w-4" />
				</Button>
			</div>
		</div>
	</Card.Content>
</Card.Root>

<AlertDialog.Root bind:open={createNamespaceDialogOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Create Namespace</AlertDialog.Title>
			<AlertDialog.Description>
				Enter a name for the new namespace. It will be created when you upload your first object.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<div class="grid gap-2 py-4">
			<div class="grid gap-1.5">
				<Label for="new-namespace-name">Namespace Name</Label>
				<Input
					id="new-namespace-name"
					bind:value={newNamespaceName}
					placeholder="e.g. production-configs"
					onkeydown={(e) => e.key === 'Enter' && handleCreateNamespace()}
				/>
			</div>
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={() => (newNamespaceName = '')}>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleCreateNamespace}>Create</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
