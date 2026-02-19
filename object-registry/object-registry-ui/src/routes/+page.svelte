<script lang="ts">
	import { browser } from '$app/environment';
	import { goto, invalidateAll } from '$app/navigation';
	import { page } from '$app/state';
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
	import { Button } from '$lib/components/ui/button';
	import { toast } from 'svelte-sonner';
	import { RefreshCw, History } from '@lucide/svelte';
	import { downloadBlob } from '$lib/utils';

	// Custom components
	import NamespaceSelector from '$lib/components/NamespaceSelector.svelte';
	import ObjectsTable from '$lib/components/ObjectsTable.svelte';
	import EventsTable from '$lib/components/EventsTable.svelte';
	import EventDialog from '$lib/components/EventDialog.svelte';
	import DeleteConfirmDialog from '$lib/components/DeleteConfirmDialog.svelte';

	let { data } = $props();

	let addedNamespaces: string[] = $state([]);
	let namespaces: string[] = $derived([...(data.namespaces || []), ...addedNamespaces]);
	// svelte-ignore state_referenced_locally
	let namespace = $state(page.url.searchParams.get('ns') || data.namespaces?.[0] || 'default');

	$effect(() => {
		if (browser && namespace) {
			const url = new URL(page.url);
			if (url.searchParams.get('ns') !== namespace) {
				url.searchParams.set('ns', namespace);
				goto(url, { replaceState: true, keepFocus: true, noScroll: true });
			}
		}
	});

	let objects: ObjectMetadata[] = $state([]);
	let events: EventResponse[] = $state([]);
	let loading = $state(true);
	let loadingEvents = $state(true);
	let error: string | null = $state(null);

	// Deletion state
	let deleteDialogOpen = $state(false);
	let objectToDelete: string | null = $state(null);
	let deleteEventDialogOpen = $state(false);
	let eventToDelete: string | null = $state(null);

	// Event Dialog state
	let showEventForm = $state(false);
	let editingEventId: string | null = $state(null);
	let eventDialog: ReturnType<typeof EventDialog>;

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

	async function handleUpload(file: File) {
		loading = true;
		error = null;
		try {
			await uploadObject(namespace, file.name, file);
			toast.success(`Successfully uploaded ${file.name}`);
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
			downloadBlob(blob, filename);
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
		try {
			await deleteObject(namespace, key);
			toast.success(`Successfully deleted ${key}`);
			await fetchObjects();
		} catch (err: any) {
			toast.error(err.message || 'Failed to delete object');
		} finally {
			loading = false;
		}
	}

	async function handleSaveEvent(req: EventRequest) {
		loadingEvents = true;
		try {
			if (editingEventId) {
				await updateEvent(namespace, editingEventId, req);
				toast.success('Successfully updated event');
			} else {
				await createEvent(namespace, req);
				toast.success('Successfully created event');
			}
			showEventForm = false;
			editingEventId = null;
			await fetchEvents();
		} catch (err: any) {
			toast.error(err.message || 'Failed to save event');
		} finally {
			loadingEvents = false;
		}
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

	$effect(() => {
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
</script>

<svelte:head>
	<title>Object Registry</title>
</svelte:head>

<div class="container mx-auto max-w-7xl space-y-8 px-4 py-10 sm:px-6 lg:px-8">
	<div class="flex items-center justify-between">
		<h1 class="text-3xl font-bold tracking-tight">Object Registry</h1>
		<div class="flex items-center gap-2">
			<Button variant="outline" href="/audit" class="flex items-center gap-2">
				<History class="h-4 w-4" />
				Audit
			</Button>
			<Button
				variant="outline"
				size="icon"
				onclick={async () => {
					await invalidateAll();
					fetchObjects();
					fetchEvents();
				}}
				disabled={loading || loadingEvents}
			>
				<RefreshCw class="h-4 w-4 {loading || loadingEvents ? 'animate-spin' : ''}" />
			</Button>
		</div>
	</div>

	<NamespaceSelector
		{namespaces}
		bind:namespace
		onNamespaceAdded={(name: string) => (addedNamespaces = [...addedNamespaces, name])}
	/>

	<ObjectsTable
		{namespace}
		{objects}
		{loading}
		{error}
		onUpload={handleUpload}
		onDownload={handleDownload}
		onDelete={confirmDelete}
	/>

	<EventsTable
		{namespace}
		{events}
		loading={loadingEvents}
		onAddEvent={() => {
			editingEventId = null;
			eventDialog?.reset();
			showEventForm = true;
		}}
		onEditEvent={(event: EventResponse) => {
			editingEventId = event.id;
			eventDialog?.reset(event);
			showEventForm = true;
		}}
		onDeleteEvent={confirmDeleteEvent}
	/>
</div>

<EventDialog
	bind:this={eventDialog}
	bind:open={showEventForm}
	{namespace}
	bind:editingEventId
	onSave={handleSaveEvent}
	onCancel={() => {
		showEventForm = false;
		editingEventId = null;
	}}
	loading={loadingEvents}
/>

<DeleteConfirmDialog
	bind:open={deleteEventDialogOpen}
	title="Delete Event Notification"
	description="Are you sure you want to delete this event? This cannot be undone."
	itemName={eventToDelete}
	onConfirm={handleDeleteEvent}
/>

<DeleteConfirmDialog
	bind:open={deleteDialogOpen}
	itemName={objectToDelete}
	onConfirm={handleDelete}
/>

<style>
	:global(body) {
		background-color: hsl(var(--background));
		color: hsl(var(--foreground));
	}
</style>
