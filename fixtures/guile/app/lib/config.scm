(define-module (app lib config)
  #:export (load-config config-get))

(define* (load-config path #:key (defaults '()) #:allow-other-keys)
  "Load the configuration at PATH, falling back to DEFAULTS."
  (if (file-exists? path)
      (call-with-input-file path read)
      defaults))

(define (config-get config key . default)
  (or (assq-ref config key)
      (and (pair? default) (car default))))
