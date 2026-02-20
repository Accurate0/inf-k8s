// src/lib/api.ts
const BASE_URL = '/api/object-registry'; // Our SvelteKit proxy endpoint

export interface MetadataResponse {
	namespace: string;
	checksum: string;
	size: number;
	content_type: string;
	created_by: string;
	created_at: string;
	labels: Record<string, string>;
}

export interface ObjectMetadata {
	key: string;
	metadata: MetadataResponse;
}

export interface ListObjectsResponse {
	objects: ObjectMetadata[];
}

export interface ObjectResponse {
	key: string;
	is_base64_encoded: boolean;
	payload: any; // Can be JSON, YAML, or Base64 string
	metadata: MetadataResponse;
}

async function handleError(response: Response, action: string): Promise<never> {
	let message = `Failed to ${action}: ${response.statusText}`;
	try {
		const contentType = response.headers.get('content-type');
		if (contentType && contentType.includes('application/json')) {
			const errorBody = await response.json();
			message = errorBody.error || errorBody.message || JSON.stringify(errorBody);
		} else {
			const textBody = await response.text();
			if (textBody) {
				message = textBody;
			}
		}
	} catch (e) {
		// If parsing fails, stick with the default message
	}
	throw new Error(message);
}

export async function listObjects(namespace: string): Promise<ObjectMetadata[]> {
	const response = await fetch(`${BASE_URL}/${namespace}`);
	if (!response.ok) {
		await handleError(response, 'list objects');
	}
	const data: ListObjectsResponse = await response.json();
	return data.objects;
}

export async function listNamespaces(): Promise<string[]> {
	const response = await fetch(`${BASE_URL}/namespaces`);
	if (!response.ok) {
		await handleError(response, 'list namespaces');
	}
	return await response.json();
}

export async function downloadObject(namespace: string, key: string): Promise<{ blob: Blob; filename: string }> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`);
	if (!response.ok) {
		await handleError(response, 'download object');
	}

	const data: ObjectResponse = await response.json();
	let blob: Blob;

	if (data.is_base64_encoded) {
		const binaryString = window.atob(data.payload);
		const bytes = new Uint8Array(binaryString.length);
		for (let i = 0; i < binaryString.length; i++) {
			bytes[i] = binaryString.charCodeAt(i);
		}
		blob = new Blob([bytes], { type: data.metadata.content_type });
	} else if (typeof data.payload === 'object') {
		blob = new Blob([JSON.stringify(data.payload, null, 2)], { type: 'application/json' });
	} else {
		blob = new Blob([data.payload], { type: data.metadata.content_type });
	}

	const filename = key.split('/').pop() || 'download';
	return { blob, filename };
}

export async function uploadObject(namespace: string, key: string, file: File): Promise<void> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`, {
		method: 'PUT',
		body: file,
		headers: {
			'Content-Type': file.type || 'application/octet-stream'
		}
	});

	if (!response.ok) {
		await handleError(response, 'upload object');
	}
}

export async function deleteObject(namespace: string, key: string): Promise<void> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`, {
		method: 'DELETE'
	});

	if (!response.ok) {
		await handleError(response, 'delete object');
	}
}

export interface NotifyResponse {
	type: string;
	method: string;
	urls: string[];
}

export interface EventResponse {
	namespace: string;
	id: string;
	keys: string[];
	notify: NotifyResponse;
	audience: string;
	created_at: string;
}

export async function listEvents(namespace: string): Promise<EventResponse[]> {
	const response = await fetch(`${BASE_URL}/events/${namespace}`);
	if (!response.ok) {
		await handleError(response, 'list events');
	}
	return await response.json();
}

export interface EventRequest {
	keys: string[];
	notify: {
		type: string;
		method: string;
		urls: string[];
	};
	audience: string;
}

export async function createEvent(namespace: string, event: EventRequest): Promise<{ id: string }> {
	const response = await fetch(`${BASE_URL}/events/${namespace}`, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json'
		},
		body: JSON.stringify(event)
	});

	if (!response.ok) {
		await handleError(response, 'create event');
	}
	return await response.json();
}

export async function deleteEvent(namespace: string, id: string): Promise<void> {
	const response = await fetch(`${BASE_URL}/events/${namespace}/${id}`, {
		method: 'DELETE'
	});

	if (!response.ok) {
		await handleError(response, 'delete event');
	}
}

export async function updateEvent(namespace: string, id: string, event: EventRequest): Promise<{ id: string }> {
	const response = await fetch(`${BASE_URL}/events/${namespace}/${id}`, {
		method: 'PUT',
		headers: {
			'Content-Type': 'application/json'
		},
		body: JSON.stringify(event)
	});

	if (!response.ok) {
		await handleError(response, 'update event');
	}
	return await response.json();
}

export interface AuditLog {
	id: string;
	timestamp: number;
	ttl: number;
	action: string;
	subject: string;
	namespace?: string;
	object_key?: string;
	details: Record<string, string>;
}

export async function listAuditLogs(
	limit: number = 100,
	actions?: string[],
	subjects?: string[],
	namespaces?: string[]
): Promise<AuditLog[]> {
	const params = new URLSearchParams();
	params.set('limit', limit.toString());

	if (actions) {
		actions.forEach((a) => params.append('action', a));
	}
	if (subjects) {
		subjects.forEach((s) => params.append('subject', s));
	}
	if (namespaces) {
		namespaces.forEach((n) => params.append('namespace', n));
	}

	const response = await fetch(`${BASE_URL}/audit?${params.toString()}`);
	if (!response.ok) {
		await handleError(response, 'list audit logs');
	}
	return await response.json();
}
