;;; Loading this module never returns.  Used to verify worker timeouts.
(define-module (pathological hangs-on-load))
(let loop () (loop))
