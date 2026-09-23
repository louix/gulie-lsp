;;; Second copy of hangs-on-load: Guile registers a module before running its
;;; body, so an interrupted load resolves instantly the next time.
(define-module (pathological hangs-on-load-again))
(let loop () (loop))
