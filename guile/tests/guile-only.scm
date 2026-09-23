;;; Guile-only metadata tests.  Guix must not be required.
;;;   guile -L guile -L fixtures/guile guile/tests/guile-only.scm
(use-modules (srfi srfi-64) (srfi srfi-1) (guile-lsp metadata))

(define (binding exports name)
  (or (find (lambda (b) (eq? (assq-ref b 'name) name)) (assq-ref exports 'bindings))
      (error "binding not reported" name)))

(define (location-file location) (and location (assq-ref location 'file)))

(test-begin "guile-only-metadata")
(define runner (test-runner-current))

(test-assert "module-exports reports the multi-module app with deduplicated imports"
  (let ((e (module-exports '(app main))))
    (and (string-suffix? "fixtures/guile/app/main.scm" (assq-ref e 'file))
         (equal? (assq-ref e 'imports) '((guile) (app lib util) (app lib config) (srfi srfi-1)))
         (equal? (assq-ref (binding e 'run) 'documentation)
                 "Run the application and return its exit status."))))

(test-equal "keyword and rest arguments are reported"
  '((required host) (optional port) (keyword timeout tls?) (allow-other-keys? . #f) (rest . extra))
  (assq-ref (binding (module-exports '(lambda-star)) 'connect) 'arguments))

(test-assert "procedure locations are 0-based file positions in the defining file"
  (let ((loc (assq-ref (binding (module-exports '(lambda-star)) 'connect) 'location)))
    (and (string-suffix? "fixtures/guile/lambda-star.scm" (location-file loc))
         (= (assq-ref loc 'line) 5)
         (= (assq-ref loc 'column) 0))))

(test-assert "macros of all three definition styles get locations"
  (let ((e (module-exports '(macros))))
    (every (lambda (name)
             (let ((b (binding e name)))
               (and (eq? (assq-ref b 'kind) 'macro)
                    (string-suffix? "macros.scm" (location-file (assq-ref b 'location))))))
           '(swap! unless* while))))

(test-equal "exported record types report their fields"
  '(x y)
  (assq-ref (binding (module-exports '(records)) '<point>) 'fields))

(test-equal "SRFI-9 records are readable from source without loading"
  '(<point> <segment>)
  (map (lambda (r) (assq-ref r 'type))
       (assq-ref (source-records "fixtures/guile/records.scm") 'records)))

(test-assert "Guix record syntax is readable from source without Guix"
  (let* ((result (source-records "fixtures/guix/my/services.scm"))
         (records (assq-ref result 'records))
         (mirror (find (lambda (r) (eq? (assq-ref r 'type) '<my-mirror>)) records))
         (label (find (lambda (f) (eq? (assq-ref f 'name) 'label)) (assq-ref mirror 'fields))))
    (and (not (assq-ref result 'read-error))
         (eq? (assq-ref mirror 'this) 'this-my-mirror)
         (assq-ref label 'thunked?)
         (equal? (assq-ref label 'default)
                 "(string-append \"mirror:\" (my-mirror-url this-my-mirror))")
         (equal? (map (lambda (f) (assq-ref f 'required?)) (assq-ref mirror 'fields))
                 '(#t #f #f #f)))))

(test-assert "define-configuration docs and defaults are readable from source"
  (let* ((records (assq-ref (source-records "fixtures/guix/my/services.scm") 'records))
         (config (find (lambda (r) (eq? (assq-ref r 'kind) 'guix-configuration)) records))
         (name (car (assq-ref config 'fields))))
    (and (eq? (assq-ref name 'type) 'string)
         (equal? (assq-ref name 'default) "\"world\"")
         (equal? (assq-ref name 'documentation) "Who to greet."))))

(test-assert "gexp reader syntax does not abort source reading"
  (not (assq-ref (source-records "fixtures/guix/gexp.scm") 'read-error)))

(test-assert "incomplete files report the read error and keep earlier forms"
  (let ((result (source-records "fixtures/guile/incomplete.scm")))
    (and (string? (assq-ref result 'read-error))
         (null? (assq-ref result 'records)))))

(test-assert "case-lambda procedures report every clause"
  (let* ((module (resolve-module '(srfi srfi-1)))
         (variable (module-variable module 'fold))
         (info (binding-info 'fold (variable-ref variable) #:module module #:variable variable))
         (arities (assq-ref info 'arities)))
    (and (= (length arities) 3)
         (equal? (assq-ref (car arities) 'required) '(kons knil list1))
         (eq? (assq-ref (caddr arities) 'rest) 'rest))))

(test-assert "variables and record types get locations by reading their file"
  (let* ((module (resolve-module '(records)))
         (variable (module-variable module '<point>))
         (info (binding-info '<point> (variable-ref variable) #:module module #:variable variable))
         (location (assq-ref info 'location)))
    (and (eq? (assq-ref info 'kind) 'record-type)
         (string-suffix? "fixtures/guile/records.scm" (assq-ref location 'file))
         (= (assq-ref location 'line) 7))))

(test-assert "a module that fails during load reports an error instead of crashing"
  (catch #t
    (lambda () (module-exports '(pathological fails-on-load)) #f)
    (lambda _ #t)))

(test-assert "a missing module is an error"
  (catch #t
    (lambda () (module-exports '(no such module)) #f)
    (lambda _ #t)))

(test-end "guile-only-metadata")
(exit (if (zero? (test-runner-fail-count runner)) 0 1))
