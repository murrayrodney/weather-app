import type { HandleClientError } from '@sveltejs/kit';
import { ApiError, userMessage } from '$lib/api/errors';

// Generic message only: never surface error internals, request URLs or server text.
export const handleError: HandleClientError = ({ error }) => ({
  message: error instanceof ApiError ? userMessage(error) : 'Something went wrong. Try again.'
});
