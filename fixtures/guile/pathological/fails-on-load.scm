;;; Loading this module raises during initialization.
(define-module (pathological fails-on-load)
  #:export (never-defined))
(error "fixture failure during module initialization")
