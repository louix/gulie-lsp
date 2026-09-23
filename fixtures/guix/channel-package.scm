;;; Package inheritance and an import from a custom channel.
(define-module (fixture channel-package)
  #:use-module (guix packages)
  #:use-module (guix gexp)
  #:use-module (gnu packages admin)
  #:use-module ((nongnu packages linux) #:prefix nongnu:)
  #:export (smartmontools-tweaked))

(define-public smartmontools-tweaked
  (package
    (inherit smartmontools)
    (name "smartmontools-tweaked")
    (arguments
     (substitute-keyword-arguments (package-arguments smartmontools)
       ((#:phases phases #~%standard-phases)
        #~(modify-phases #$phases
            (add-after 'install 'note
              (lambda _ (display "tweaked\n")))))))
    (synopsis "smartmontools with a local tweak")))

(define firmware-list
  (list nongnu:linux-firmware))
