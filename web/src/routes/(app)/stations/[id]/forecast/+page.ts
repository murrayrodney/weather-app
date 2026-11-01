import { redirect } from '@sveltejs/kit';
import { forecastRedirectTarget } from '$lib/transforms/forecast';
import type { PageLoad } from './$types';

// Phase 1 of proposal 07: the forecast lives on the station page (#forecast). Kept so bookmarks still work.
export const load: PageLoad = ({ params }) => {
  throw redirect(308, forecastRedirectTarget(params.id));
};
