<script lang="ts">
	import { browser } from '$app/environment';
	import { untrack } from 'svelte';
	import {
		listObjects,
		downloadObject,
		uploadObject,
		deleteObject,
		listEvents,
		createEvent,
		deleteEvent,
		updateEvent,
		type ObjectMetadata,
		type EventResponse,
		type EventRequest
	} from '$lib/api';
	import * as Card from '$lib/components/ui/card';
	import * as Select from '$lib/components/ui/select';
	import * as Table from '$lib/components/ui/table';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import { Input } from '$lib/components/ui/input';
	import { Separator } from '$lib/components/ui/separator';
	import { toast } from 'svelte-sonner';
	import {
		Loader2,
		Download,
		Upload,
		RefreshCw,
		FileText,
		Trash2,
		Plus,
		Bell,
		Pencil,
		X
	} from '@lucide/svelte';

	let { data } = $props();

	let addedNamespaces: string[] = $state([]);
	let namespaces: string[] = $derived([...(data.namespaces || []), ...addedNamespaces]);
	// svelte-ignore state_referenced_locally
	let namespace = $state(data.namespaces?.[0] || 'default');

	let objects: ObjectMetadata[] = $state([]);
	let events: EventResponse[] = $state([]);
	let loading = $state(true);
	let loadingEvents = $state(true);
	let error: string | null = $state(null);
	let fileInput: HTMLInputElement | null = $state(null);
	let selectedFile: File | null = $state(null);

	// AlertDialog state
	let deleteDialogOpen = $state(false);
	let objectToDelete: string | null = $state(null);

	// Create Namespace state
	let createNamespaceDialogOpen = $state(false);
	let newNamespaceName = $state('');

	// Create Event state
	let showEventForm = $state(false);
	let editingEventId: string | null = $state(null);
	let newEventKeys = $state(['*']);
	let newEventNotifyType = $state('HTTP');
	let newEventNotifyMethod = $state('POST');
	let newEventNotifyUrls = $state(['']);
	let newEventAudience = $state('object-registry');

	// Delete Event state
	let deleteEventDialogOpen = $state(false);
	let eventToDelete: string | null = $state(null);

	async function fetchObjects() {
		if (!browser) return;
		loading = true;
		error = null;
		try {
			const fetchedObjects = await listObjects(namespace);
			objects = fetchedObjects;
		} catch (err: any) {
			const msg = err.message || 'Failed to fetch objects';
			error = msg;
			toast.error(msg);
		} finally {
			loading = false;
		}
	}

	async function fetchEvents() {
		if (!browser) return;
		loadingEvents = true;
		try {
			const fetchedEvents = await listEvents(namespace);
			events = fetchedEvents;
		} catch (err: any) {
			console.error('Failed to fetch events:', err);
		} finally {
			loadingEvents = false;
		}
	}

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
		addedNamespaces = [...addedNamespaces, name];
		namespace = name;
		newNamespaceName = '';
		createNamespaceDialogOpen = false;
		toast.success(`Namespace '${name}' added to local view`);
	}

	async function handleUpload() {
		if (!selectedFile) {
			toast.warning('Please select a file to upload.');
			return;
		}
		loading = true;
		error = null;
		try {
			await uploadObject(namespace, selectedFile.name, selectedFile);
			toast.success(`Successfully uploaded ${selectedFile.name}`);
			selectedFile = null;
			if (fileInput) fileInput.value = '';
			await fetchObjects();
		} catch (err: any) {
			const msg = err.message || 'Failed to upload file';
			error = msg;
			toast.error(msg);
		} finally {
			loading = false;
		}
	}

	async function handleDownload(key: string) {
		loading = true;
		error = null;
		try {
			const { blob, filename } = await downloadObject(namespace, key);
			const url = window.URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = filename;
			document.body.appendChild(a);
			a.click();
			a.remove();
			window.URL.revokeObjectURL(url);
			toast.success(`Downloading ${filename}`);
		} catch (err: any) {
			const msg = err.message || 'Failed to download file';
			error = msg;
			toast.error(msg);
		} finally {
			loading = false;
		}
	}

	function confirmDelete(key: string) {
		objectToDelete = key;
		deleteDialogOpen = true;
	}

	async function handleDelete() {
		if (!objectToDelete) return;

		const key = objectToDelete;
		deleteDialogOpen = false;
		objectToDelete = null;

		loading = true;
		error = null;
		try {
			await deleteObject(namespace, key);
			toast.success(`Successfully deleted ${key}`);
			await fetchObjects();
		} catch (err: any) {
			const msg = err.message || 'Failed to delete object';
			error = msg;
			toast.error(msg);
		} finally {
			loading = false;
		}
	}

	async function handleSaveEvent() {
		const keys = newEventKeys.map((k) => k.trim()).filter(Boolean);
		const urls = newEventNotifyUrls.map((u) => u.trim()).filter(Boolean);

		if (keys.length === 0 || urls.length === 0 || !newEventAudience.trim()) {
			toast.warning('Please fill in all required fields');
			return;
		}

		loadingEvents = true;
		try {
			const req: EventRequest = {
				keys,
				notify: {
					type: newEventNotifyType,
					method: newEventNotifyMethod,
					urls
				},
				audience: newEventAudience.trim()
			};

			if (editingEventId) {
				await updateEvent(namespace, editingEventId, req);
				toast.success('Successfully updated event');
			} else {
				await createEvent(namespace, req);
				toast.success('Successfully created event');
			}

			showEventForm = false;
			editingEventId = null;
			// Reset form
			newEventKeys = ['*'];
			newEventNotifyUrls = [''];
			newEventNotifyType = 'HTTP';
			newEventNotifyMethod = 'POST';
			newEventAudience = 'object-registry';
			await fetchEvents();
		} catch (err: any) {
			toast.error(err.message || 'Failed to save event');
		} finally {
			loadingEvents = false;
		}
	}

	function startEditingEvent(event: EventResponse) {
		editingEventId = event.id;
		newEventKeys = [...event.keys];
		newEventNotifyType = event.notify.type;
		newEventNotifyMethod = event.notify.method;
		newEventNotifyUrls = [...event.notify.urls];
		newEventAudience = 'object-registry';
		showEventForm = true;
	}

	function cancelEditingEvent() {
		showEventForm = false;
		editingEventId = null;
		newEventKeys = ['*'];
		newEventNotifyUrls = [''];
		newEventNotifyType = 'HTTP';
		newEventNotifyMethod = 'POST';
		newEventAudience = 'object-registry';
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

	function confirmDeleteEvent(id: string) {
		eventToDelete = id;
		deleteEventDialogOpen = true;
	}

	async function handleDeleteEvent() {
		if (!eventToDelete) return;

		const id = eventToDelete;
		deleteEventDialogOpen = false;
		eventToDelete = null;

		loadingEvents = true;
		try {
			await deleteEvent(namespace, id);
			toast.success(`Successfully deleted event ${id}`);
			await fetchEvents();
		} catch (err: any) {
			toast.error(err.message || 'Failed to delete event');
		} finally {
			loadingEvents = false;
		}
	}

	function handleFileChange(event: Event) {
		const target = event.target as HTMLInputElement;
		selectedFile = target.files ? target.files[0] : null;
	}

	function formatSize(bytes: number): string {
		if (bytes === 0) return '0 B';
		const k = 1024;
		const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}

	$effect(() => {
		// Only run this when data.namespaces changes
		if (data.namespaces) {
			const filtered = untrack(() => addedNamespaces).filter(
				(ns) => !data.namespaces.includes(ns)
			);
			if (filtered.length !== addedNamespaces.length) {
				addedNamespaces = filtered;
			}
		}
	});

	$effect(() => {
		if (browser && namespace) {
			fetchObjects();
			fetchEvents();
		}
	});

	let selectedNamespaceLabel = $derived(namespaces.find((ns) => ns === namespace) || namespace);
</script>

<div class="container mx-auto max-w-7xl space-y-8 py-10">
	<div class="flex items-center justify-between">
		<h1 class="text-3xl font-bold tracking-tight">Object Registry</h1>
		<Button
			variant="outline"
			size="icon"
			onclick={() => {
				fetchObjects();
				fetchEvents();
			}}
			disabled={loading || loadingEvents}
		>
			<RefreshCw class="h-4 w-4 {loading || loadingEvents ? 'animate-spin' : ''}" />
		</Button>
	</div>

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

	<div class="space-y-4">
		<h2 class="text-2xl font-semibold tracking-tight">
			Objects in <span class="text-primary">{namespace}</span>
		</h2>

		<Card.Root>
			<Card.Content class="p-0">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="pl-6">Key</Table.Head>
							<Table.Head>Size</Table.Head>
							<Table.Head class="hidden md:table-cell">Content Type</Table.Head>
							<Table.Head class="hidden lg:table-cell">Created By</Table.Head>
							<Table.Head class="hidden sm:table-cell">Last Modified</Table.Head>
							<Table.Head class="pr-6 text-right">Actions</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#if loading && objects.length === 0}
							<Table.Row>
								<Table.Cell colspan={6} class="h-32 text-center">
									<div class="flex flex-col items-center justify-center space-y-2">
										<Loader2 class="h-8 w-8 animate-spin text-muted-foreground" />
										<p class="animate-pulse text-muted-foreground">Loading objects...</p>
									</div>
								</Table.Cell>
							</Table.Row>
						{:else if objects.length === 0 && !error}
							<Table.Row>
								<Table.Cell colspan={6} class="h-32 text-center">
									<div class="flex flex-col items-center justify-center space-y-2">
										<FileText class="h-12 w-12 text-muted-foreground/50" />
										<p class="text-muted-foreground">No objects found in this namespace.</p>
									</div>
								</Table.Cell>
							</Table.Row>
						{:else}
							{#each objects as object (object.key)}
								<Table.Row>
									<Table.Cell class="pl-6 font-medium">{object.key}</Table.Cell>
									<Table.Cell>{formatSize(object.metadata.size)}</Table.Cell>
									<Table.Cell class="hidden text-muted-foreground md:table-cell"
										>{object.metadata.content_type}</Table.Cell
									>
									<Table.Cell class="hidden lg:table-cell">{object.metadata.created_by}</Table.Cell>
									<Table.Cell class="hidden text-muted-foreground sm:table-cell">
										{new Date(object.metadata.created_at).toLocaleString()}
									</Table.Cell>
									<Table.Cell class="pr-6 text-right">
										<div class="flex justify-end gap-2">
											<Button
												variant="ghost"
												size="sm"
												onclick={() => handleDownload(object.key)}
												disabled={loading}
											>
												<Download class="mr-2 h-4 w-4" />
												Download
											</Button>
											<Button
												variant="ghost"
												size="sm"
												onclick={() => confirmDelete(object.key)}
												disabled={loading}
												class="text-destructive hover:bg-destructive/10"
											>
												<Trash2 class="mr-2 h-4 w-4" />
												Delete
											</Button>
										</div>
									</Table.Cell>
								</Table.Row>
							{/each}
						{/if}

						<!-- Add File Row -->
						<Table.Row
							class="group cursor-pointer transition-colors hover:bg-muted/50"
							onclick={() => fileInput?.click()}
						>
							<Table.Cell class="px-6 py-4" colspan={6}>
								<div
									class="flex items-center gap-3 text-muted-foreground transition-colors group-hover:text-foreground"
								>
									<div
										class="flex h-8 w-8 items-center justify-center rounded-lg border-2 border-dashed border-muted-foreground/25 transition-all group-hover:border-primary/50 group-hover:bg-primary/5"
									>
										{#if loading && selectedFile}
											<Loader2 class="h-4 w-4 animate-spin" />
										{:else if selectedFile}
											<FileText class="h-4 w-4 text-primary" />
										{:else}
											<Plus class="h-4 w-4" />
										{/if}
									</div>
									<span class="text-sm font-medium">
										{#if selectedFile}
											<span class="font-semibold text-foreground">{selectedFile.name}</span>
											<span class="ml-2 text-xs text-muted-foreground"
												>({formatSize(selectedFile.size)})</span
											>
										{:else}
											Click to add a file to this namespace...
										{/if}
									</span>
									<input
										id="file-upload"
										type="file"
										onchange={handleFileChange}
										bind:this={fileInput}
										class="hidden"
									/>
									{#if selectedFile}
										<Button
											onclick={(e) => {
												e.stopPropagation();
												handleUpload();
											}}
											disabled={loading}
											size="sm"
											class="ml-4"
										>
											{#if loading}
												<Loader2 class="mr-2 h-4 w-4 animate-spin" />
												Uploading
											{:else}
												<Upload class="mr-2 h-4 w-4" />
												Confirm Upload
											{/if}
										</Button>
									{/if}
								</div>
							</Table.Cell>
						</Table.Row>
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	</div>

	<div class="space-y-4">
		<div class="flex items-center justify-between">
			<h2 class="text-2xl font-semibold tracking-tight">
				Events in <span class="text-primary">{namespace}</span>
			</h2>
		</div>

		<Card.Root>
			<Card.Content class="p-0">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="pl-6">ID</Table.Head>
							<Table.Head>Keys</Table.Head>
							<Table.Head>Notification</Table.Head>
							<Table.Head>Created At</Table.Head>
							<Table.Head class="pr-6 text-right">Actions</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#if loadingEvents && events.length === 0}
							<Table.Row>
								<Table.Cell colspan={5} class="h-32 text-center">
									<div class="flex flex-col items-center justify-center space-y-2">
										<Loader2 class="h-8 w-8 animate-spin text-muted-foreground" />
										<p class="animate-pulse text-muted-foreground">Loading events...</p>
									</div>
								</Table.Cell>
							</Table.Row>
						{:else if events.length === 0}
							<Table.Row>
								<Table.Cell colspan={5} class="h-32 text-center">
									<div class="flex flex-col items-center justify-center space-y-2">
										<Bell class="h-12 w-12 text-muted-foreground/50" />
										<p class="text-muted-foreground">No events found in this namespace.</p>
									</div>
								</Table.Cell>
							</Table.Row>
						{:else}
							{#each events as event (event.id)}
								<Table.Row>
									<Table.Cell class="pl-6 font-mono text-xs font-medium">{event.id}</Table.Cell>
									<Table.Cell>
										<div class="flex flex-wrap gap-1">
											{#each event.keys as key}
												<span
													class="inline-flex items-center rounded-md bg-muted px-2 py-1 text-xs font-medium ring-1 ring-gray-500/10 ring-inset"
												>
													{key}
												</span>
											{/each}
										</div>
									</Table.Cell>
									<Table.Cell>
										<div class="flex flex-col gap-1 text-xs">
											<div class="flex items-center gap-2">
												<span
													class="rounded bg-primary/10 px-1 text-[10px] font-semibold text-primary uppercase"
												>
													{event.notify.type}
												</span>
												<span class="text-muted-foreground italic">
													{event.notify.method}
												</span>
											</div>
											<div
												class="max-w-[300px] truncate text-muted-foreground"
												title={event.notify.urls.join(', ')}
											>
												{event.notify.urls.join(', ')}
											</div>
										</div>
									</Table.Cell>
									<Table.Cell class="text-sm text-muted-foreground">
										{new Date(event.created_at).toLocaleString()}
									</Table.Cell>
									<Table.Cell class="pr-6 text-right">
										<div class="flex justify-end gap-1">
											<Button
												variant="ghost"
												size="sm"
												onclick={() => startEditingEvent(event)}
												disabled={loadingEvents}
												class="h-8 w-8 p-0"
											>
												<Pencil class="h-4 w-4" />
											</Button>
											<Button
												variant="ghost"
												size="sm"
												onclick={() => confirmDeleteEvent(event.id)}
												disabled={loadingEvents}
												class="h-8 w-8 p-0 text-destructive hover:bg-destructive/10"
											>
												<Trash2 class="h-4 w-4" />
											</Button>
										</div>
									</Table.Cell>
								</Table.Row>
							{/each}
						{/if}

						<!-- Add Event Row -->
						<Table.Row
							class="group cursor-pointer transition-colors hover:bg-muted/50"
							onclick={() => {
								cancelEditingEvent();
								showEventForm = true;
							}}
						>
							<Table.Cell class="px-6 py-4" colspan={5}>
								<div
									class="flex items-center gap-3 text-muted-foreground transition-colors group-hover:text-foreground"
								>
									<div
										class="flex h-8 w-8 items-center justify-center rounded-lg border-2 border-dashed border-muted-foreground/25 transition-all group-hover:border-primary/50 group-hover:bg-primary/5"
									>
										<Plus class="h-4 w-4" />
									</div>
									<span class="text-sm font-medium"
										>Click to configure a new event notification for this namespace...</span
									>
								</div>
							</Table.Cell>
						</Table.Row>
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	</div>
</div>

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

<AlertDialog.Root bind:open={showEventForm}>
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
						<Button variant="outline" size="icon" onclick={() => removeKey(i)} disabled={newEventKeys.length <= 1}>
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
						<Button variant="outline" size="icon" onclick={() => removeUrl(i)} disabled={newEventNotifyUrls.length <= 1}>
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
			<AlertDialog.Cancel onclick={cancelEditingEvent}>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleSaveEvent} disabled={loadingEvents}>
				{#if loadingEvents}
					<Loader2 class="mr-2 h-4 w-4 animate-spin" />
				{/if}
				{editingEventId ? 'Update' : 'Create'} Event
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={deleteEventDialogOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Delete Event Notification</AlertDialog.Title>
			<AlertDialog.Description>
				Are you sure you want to delete event <strong>{eventToDelete}</strong>? This cannot be undone.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				onclick={handleDeleteEvent}
				class="bg-destructive text-destructive-foreground hover:bg-destructive/90"
			>
				Delete
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={deleteDialogOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Are you absolutely sure?</AlertDialog.Title>
			<AlertDialog.Description>
				This action cannot be undone. This will permanently delete <strong>{objectToDelete}</strong> from
				the registry.
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Cancel</AlertDialog.Cancel>
			<AlertDialog.Action
				onclick={handleDelete}
				class="bg-destructive text-destructive-foreground hover:bg-destructive/90"
			>
				Delete
			</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<style>
	:global(body) {
		background-color: hsl(var(--background));
		color: hsl(var(--foreground));
	}
</style>
