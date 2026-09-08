//! One request's method, and what that method serves: the `GET` gate
//! ([`get`]) and its route table ([`get_routes`]), the `POST` gate
//! ([`post`]), and the `PUT`/`DELETE` gates ([`write`]).

mod get;
mod get_routes;
mod post;
mod write;
