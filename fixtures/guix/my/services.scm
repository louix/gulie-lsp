;;; Local helper module: a small service defined with define-configuration
;;; and a record defined with define-record-type*.
(define-module (my services)
  #:use-module (gnu services)
  #:use-module (gnu services configuration)
  #:use-module (gnu services shepherd)
  #:use-module (guix gexp)
  #:use-module (guix records)
  #:export (my-hello-configuration
            my-hello-service-type
            my-mirror
            my-mirror?
            my-mirror-url))

(define-configuration my-hello-configuration
  (name
   (string "world")
   "Who to greet.")
  (repeat
   (integer 1)
   "How many times to greet.")
  (no-serialization))

(define (my-hello-shepherd-service config)
  (list (shepherd-service
         (provision '(my-hello))
         (documentation "Print a greeting once at boot.")
         (start #~(lambda ()
                    (let loop ((n #$(my-hello-configuration-repeat config)))
                      (unless (zero? n)
                        (display #$(string-append "Hello, " (my-hello-configuration-name config) "\n"))
                        (loop (- n 1))))
                    #t))
         (respawn? #f))))

(define my-hello-service-type
  (service-type
   (name 'my-hello)
   (description "Greet someone at boot time.")
   (extensions (list (service-extension shepherd-root-service-type
                                        my-hello-shepherd-service)))
   (default-value (my-hello-configuration))))

;; A record in the style of Guix's own: syntactic constructor plus defaults.
(define-record-type* <my-mirror> my-mirror make-my-mirror
  my-mirror?
  this-my-mirror
  (url      my-mirror-url)                       ; string, required
  (priority my-mirror-priority (default 10))     ; integer
  (label    my-mirror-label
            (thunked)
            (default (string-append "mirror:" (my-mirror-url this-my-mirror))))
  (enabled? my-mirror-enabled? (default #t) (innate)))
