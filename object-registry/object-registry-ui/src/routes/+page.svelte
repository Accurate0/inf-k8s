<script lang="ts">
	import { browser } from '$app/environment';
	import {
		listObjects,
		downloadObject,
		uploadObject,
		deleteObject,
		listEvents,
		type ObjectMetadata,
		type EventResponse
	} from '$lib/api';
	import * as Card from '$lib/components/ui/card';
	import * as Select from '$lib/components/ui/select';
	import * as Table from '$lib/components/ui/table';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import { toast } from 'svelte-sonner';
	import {
		Loader2,
		Download,
		Upload,
		RefreshCw,
		FileText,
		Trash2,
		Plus,
		Bell
	} from '@lucide/svelte';

	let { data } = $props();

	let namespaces: string[] = $derived(data.namespaces || ['default']);
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
							<Table.Cell class="pl-6" colspan={5}>
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
											Click to browse or add a file to this namespace...
										{/if}
									</span>
									<input
										id="file-upload"
										type="file"
										onchange={handleFileChange}
										bind:this={fileInput}
										class="hidden"
									/>
								</div>
							</Table.Cell>
							<Table.Cell class="pr-6 text-right">
								<Button
									onclick={(e) => {
										e.stopPropagation();
										handleUpload();
									}}
									disabled={!selectedFile || loading}
									size="sm"
									variant={selectedFile ? 'default' : 'ghost'}
									class={selectedFile ? '' : 'opacity-0 transition-opacity group-hover:opacity-100'}
								>
									{#if loading && selectedFile}
										<Loader2 class="mr-2 h-4 w-4 animate-spin" />
										Uploading
									{:else}
										<Upload class="mr-2 h-4 w-4" />
										{selectedFile ? 'Confirm Upload' : 'Upload'}
									{/if}
								</Button>
							</Table.Cell>
						</Table.Row>
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	</div>

	<div class="space-y-4">
		<h2 class="text-2xl font-semibold tracking-tight">
			Events in <span class="text-primary">{namespace}</span>
		</h2>

		<Card.Root>
			<Card.Content class="p-0">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head class="pl-6">ID</Table.Head>
							<Table.Head>Keys</Table.Head>
							<Table.Head>Notification</Table.Head>
							<Table.Head class="pr-6 text-right">Created At</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#if loadingEvents && events.length === 0}
							<Table.Row>
								<Table.Cell colspan={4} class="h-32 text-center">
									<div class="flex flex-col items-center justify-center space-y-2">
										<Loader2 class="h-8 w-8 animate-spin text-muted-foreground" />
										<p class="animate-pulse text-muted-foreground">Loading events...</p>
									</div>
								</Table.Cell>
							</Table.Row>
						{:else if events.length === 0}
							<Table.Row>
								<Table.Cell colspan={4} class="h-32 text-center">
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
									<Table.Cell class="pr-6 text-right text-sm text-muted-foreground">
										{new Date(event.created_at).toLocaleString()}
									</Table.Cell>
								</Table.Row>
							{/each}
						{/if}
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	</div>
</div>

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
