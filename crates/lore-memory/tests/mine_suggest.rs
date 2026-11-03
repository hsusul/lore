//! A mined lesson's suggested rule: the person's own words with the
//! acknowledgements, request frames, and filler cut away. Fixed rules on the
//! words, never a model. Every pair below is a real correction.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::mine::suggest_rule;

#[test]
fn acknowledgements_a_request_frame_and_a_trailing_please_are_cut() {
    assert_eq!(
        suggest_rule("yea sure, and also make sure that docs are never tracked please"),
        "Docs are never tracked"
    );
}

#[test]
fn a_question_frame_at_the_start_is_left_alone() {
    assert_eq!(
        suggest_rule("did you push any of the docs, make sure that none of the docs are pushed"),
        "Did you push any of the docs, make sure that none of the docs are pushed"
    );
}

#[test]
fn do_that_and_make_sure_that_you_are_cut() {
    assert_eq!(
        suggest_rule("yes do that, also make sure that you commit and push every little step"),
        "Commit and push every little step"
    );
}

#[test]
fn a_rule_already_clean_comes_back_as_it_is() {
    assert_eq!(
        suggest_rule("Stop asking me for permission"),
        "Stop asking me for permission"
    );
}

#[test]
fn from_now_on_is_cut_and_dont_gets_its_apostrophe() {
    assert_eq!(
        suggest_rule("dont make a doc for it just give me the thing to copy from now on"),
        "Don't make a doc for it just give me the thing to copy"
    );
}

#[test]
fn spelling_is_not_fixed() {
    assert_eq!(
        suggest_rule("never use urself as a contributor"),
        "Never use urself as a contributor"
    );
}

#[test]
fn just_like_between_words_is_cut() {
    assert_eq!(
        suggest_rule("dont reccomend something just like say the issue"),
        "Don't reccomend something say the issue"
    );
}

#[test]
fn a_request_mixed_with_a_task_comes_back_trimmed() {
    let text = "ok go ahead and rebuild and look at new image, if you do anything that \
requires storage space don't do it on my mac do it on the ssd";
    assert_eq!(suggest_rule(&format!("  {text} ")), text);
}

#[test]
fn openers_are_cut_only_as_whole_words() {
    assert_eq!(
        suggest_rule("sometimes the build breaks on ci"),
        "Sometimes the build breaks on ci"
    );
    assert_eq!(
        suggest_rule("make sure your tests pass before pushing."),
        "Your tests pass before pushing"
    );
}

#[test]
fn too_little_left_returns_the_original() {
    assert_eq!(suggest_rule(" ok, please "), "ok, please");
    assert_eq!(suggest_rule("yes do that please"), "yes do that please");
}
