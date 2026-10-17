import { HttpErrorResponse, type HttpInterceptorFn } from '@angular/common/http';
import { inject } from '@angular/core';
import { catchError, from, switchMap, throwError } from 'rxjs';
import { Auth } from './auth';

export const authInterceptor: HttpInterceptorFn = (request, next) => {
  if (!request.url.startsWith('/api')) {
    return next(request);
  }
  const auth = inject(Auth);
  return from(auth.token()).pipe(
    switchMap((token) => {
      const authed = token
        ? request.clone({ setHeaders: { Authorization: `Bearer ${token}` } })
        : request;
      return next(authed);
    }),
    catchError((error: unknown) => {
      if (error instanceof HttpErrorResponse && error.status === 401 && auth.enabled()) {
        void auth.login();
      }
      return throwError(() => error);
    }),
  );
};
