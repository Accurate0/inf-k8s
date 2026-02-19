import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ fetch }) => {
	const response = await fetch('/api/object-registry/namespaces');
	let namespaces: string[] = ['default'];

	if (response.ok) {
		try {
			namespaces = await response.json();
		} catch (err) {
			console.error('Failed to parse namespaces:', err);
		}
	} else {
		console.error('Failed to fetch namespaces:', response.statusText);
	}

	return {
		namespaces
	};
};
