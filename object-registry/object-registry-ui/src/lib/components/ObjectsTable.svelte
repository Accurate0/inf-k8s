<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Button } from '$lib/components/ui/button';
	import { Loader2, Download, Trash2, Plus, FileText, Upload } from '@lucide/svelte';
	import type { ObjectMetadata } from '$lib/api';
	import { formatSize } from '$lib/utils';

	let {
		namespace,
		objects = [],
		loading = false,
		onUpload,
		onDownload,
		onDelete,
		error = null
	} = $props();

	let fileInput: HTMLInputElement | null = $state(null);
	let selectedFile: File | null = $state(null);
	let uploading = $state(false);

	function handleFileChange(event: Event) {
		const target = event.target as HTMLInputElement;
		selectedFile = target.files ? target.files[0] : null;
	}

	async function handleUploadClick() {
		if (!selectedFile) return;
		uploading = true;
		try {
			await onUpload(selectedFile);
			selectedFile = null;
			if (fileInput) fileInput.value = '';
		} finally {
			uploading = false;
		}
	}
</script>

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
									<div class="flex justify-end gap-1">
										<Button
											variant="ghost"
											size="sm"
											onclick={() => onDownload(object.key)}
											disabled={loading}
											class="h-8 w-8 p-0"
											title="Download"
										>
											<Download class="h-4 w-4" />
										</Button>
										<Button
											variant="ghost"
											size="sm"
											onclick={() => onDelete(object.key)}
											disabled={loading}
											class="h-8 w-8 p-0 text-destructive hover:bg-destructive/10"
											title="Delete"
										>
											<Trash2 class="h-4 w-4" />
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
						<Table.Cell class="pl-6 py-4" colspan={5}>
							<div
								class="flex items-center gap-3 text-muted-foreground transition-colors group-hover:text-foreground"
							>
								<div
									class="flex h-8 w-8 items-center justify-center rounded-lg border-2 border-dashed border-muted-foreground/25 transition-all group-hover:border-primary/50 group-hover:bg-primary/5"
								>
									{#if uploading && selectedFile}
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
							</div>
						</Table.Cell>
						<Table.Cell class="pr-6 py-4 text-right">
							{#if selectedFile}
								<Button
									onclick={(e) => {
										e.stopPropagation();
										handleUploadClick();
									}}
									disabled={uploading}
									size="sm"
								>
									{#if uploading}
										<Loader2 class="mr-2 h-4 w-4 animate-spin" />
										Uploading
									{:else}
										<Upload class="mr-2 h-4 w-4" />
										Confirm Upload
									{/if}
								</Button>
							{/if}
						</Table.Cell>
					</Table.Row>
				</Table.Body>
			</Table.Root>
		</Card.Content>
	</Card.Root>
</div>
