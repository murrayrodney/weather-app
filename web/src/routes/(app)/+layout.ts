import { redirect } from '@sveltejs/kit';
import { getMe } from '$lib/api/endpoints';
import { ApiError } from '$lib/api/errors';
import { safeNext } from '$lib/auth/next';
import { authRedirectTarget } from '$lib/auth/redirect';
import { supabase } from '$lib/auth/supabase';

// UX guard only: the API enforces access. `url` is read only when redirecting, so a normal navigation does not re-run this load.
export const load = async ({ url }) => {
  const { data } = await supabase.auth.getSession();
  if (!data.session) {
    throw redirect(307, '/login?next=' + encodeURIComponent(safeNext(url.pathname + url.search, url.origin)));
  }
  try {
    return { me: await getMe() };
  } catch (err) {
    const target = authRedirectTarget(err);
    if (target) throw redirect(307, target);
    if (err instanceof ApiError) throw err;
    throw err;
  }
};
