package main

import (
	"io"
	"log"
	"net/http"
)

func hello(w http.ResponseWriter, r *http.Request) {
	io.WriteString(w, "Bye, World!\r\n")
}

func hey(w http.ResponseWriter, r *http.Request) {
	io.WriteString(w, "Buh bye!")
}

func echo(w http.ResponseWriter, r *http.Request) {
	io.Copy(w, r.Body)
}

func main() {
	mux := http.NewServeMux()
	mux.HandleFunc("/", hello)
	mux.HandleFunc("/hey", hey)
	mux.HandleFunc("/echo", echo)
	log.Fatal(http.ListenAndServe(":8080", mux))
}
