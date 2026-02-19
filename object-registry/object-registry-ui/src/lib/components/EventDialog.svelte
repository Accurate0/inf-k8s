<script lang="ts">
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as Select from '$lib/components/ui/select';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import { Input } from '$lib/components/ui/input';
	import { Separator } from '$lib/components/ui/separator';
	import { Plus, X, Loader2 } from '@lucide/svelte';
	import type { EventResponse, EventRequest } from '$lib/api';

	let {
		open = $bindable(),
		namespace,
		editingEventId = $bindable(),
		onSave,
		onCancel,
		loading = false
	} = $props();

	let newEventKeys = $state(['*']);
	let newEventNotifyType = $state('HTTP');
	let newEventNotifyMethod = $state('POST');
	let newEventNotifyUrls = $state(['']);
	let newEventAudience = $state('object-registry');

	export function reset(event?: EventResponse) {
		if (event) {
			newEventKeys = [...event.keys];
			newEventNotifyType = event.notify.type;
			newEventNotifyMethod = event.notify.method;
			newEventNotifyUrls = [...event.notify.urls];
			newEventAudience = event.audience;
		} else {
			newEventKeys = ['*'];
			newEventNotifyUrls = [''];
			newEventNotifyType = 'HTTP';
			newEventNotifyMethod = 'POST';
			newEventAudience = 'object-registry';
		}
	}

	function addKey() {
		newEventKeys = [...newEventKeys, ''];
	}

	function removeKey(index: number) {
		newEventKeys = newEventKeys.filter((_, i) => i !== index);
		if (newEventKeys.length === 0) newEventKeys = [''];
	}

	function addUrl() {
		newEventNotifyUrls = [...newEventNotifyUrls, ''];
	}

	function removeUrl(index: number) {
		newEventNotifyUrls = newEventNotifyUrls.filter((_, i) => i !== index);
		if (newEventNotifyUrls.length === 0) newEventNotifyUrls = [''];
	}

	async function handleSave() {
		const keys = newEventKeys.map((k) => k.trim()).filter(Boolean);
		const urls = newEventNotifyUrls.map((u) => u.trim()).filter(Boolean);

		const req: EventRequest = {
			keys,
			notify: {
				type: newEventNotifyType,
				method: newEventNotifyMethod,
				urls
			},
			audience: newEventAudience.trim()
		};

		await onSave(req);
	}
</script>

<AlertDialog.Root bind:open>
	<AlertDialog.Content class="max-w-2xl">
		<AlertDialog.Header>
			<AlertDialog.Title>{editingEventId ? 'Edit' : 'Create'} Event Notification</AlertDialog.Title>
			<AlertDialog.Description>
				Configure a notification for object changes in <strong>{namespace}</strong>.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<div class="grid gap-4 py-4">
			<div class="grid gap-2">
				<Label>Object Keys / Patterns</Label>
				{#each newEventKeys as _, i}
					<div class="flex gap-2">
						<Input bind:value={newEventKeys[i]} placeholder="e.g. *, data/*, config.json" />
						<Button
							variant="outline"
							size="icon"
							onclick={() => removeKey(i)}
							disabled={newEventKeys.length <= 1}
						>
							<X class="h-4 w-4" />
						</Button>
					</div>
				{/each}
				<Button variant="ghost" size="sm" onclick={addKey} class="justify-start w-fit px-2">
					<Plus class="mr-2 h-4 w-4" /> Add another key
				</Button>
			</div>

			<Separator />

			<div class="grid grid-cols-2 gap-4">
				<div class="grid gap-1.5">
					<Label for="popup-notify-type">Type</Label>
					<Select.Root type="single" bind:value={newEventNotifyType}>
						<Select.Trigger id="popup-notify-type">{newEventNotifyType}</Select.Trigger>
						<Select.Content>
							<Select.Item value="HTTP">HTTP</Select.Item>
						</Select.Content>
					</Select.Root>
				</div>
				<div class="grid gap-1.5">
					<Label for="popup-notify-method">Method</Label>
					<Select.Root type="single" bind:value={newEventNotifyMethod}>
						<Select.Trigger id="popup-notify-method">{newEventNotifyMethod}</Select.Trigger>
						<Select.Content>
							<Select.Item value="POST">POST</Select.Item>
							<Select.Item value="PUT">PUT</Select.Item>
							<Select.Item value="GET">GET</Select.Item>
						</Select.Content>
					</Select.Root>
				</div>
			</div>

			<div class="grid gap-2">
				<Label>Notification URLs</Label>
				{#each newEventNotifyUrls as _, i}
					<div class="flex gap-2">
						<Input bind:value={newEventNotifyUrls[i]} placeholder="https://..." />
						<Button
							variant="outline"
							size="icon"
							onclick={() => removeUrl(i)}
							disabled={newEventNotifyUrls.length <= 1}
						>
							<X class="h-4 w-4" />
						</Button>
					</div>
				{/each}
				<Button variant="ghost" size="sm" onclick={addUrl} class="justify-start w-fit px-2">
					<Plus class="mr-2 h-4 w-4" /> Add another URL
				</Button>
			</div>

			<div class="grid gap-1.5">
				<Label for="popup-audience">Audience (JWT Aud)</Label>
				<Input id="popup-audience" bind:value={newEventAudience} />
			</div>
		</div>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={onCancel}>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleSave} disabled={loading}>
				{#if loading}
					<Loader2 class="mr-2 h-4 w-4 animate-spin" />
				{/if}
				{editingEventId ? 'Update' : 'Create'} Event
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
