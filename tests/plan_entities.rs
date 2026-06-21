use snomed_finding_extractor::{
    extract_plan_entities, PlanEntityKind, PlanExtractRequest, SoapField,
};

fn extract(plan: &str) -> Vec<PlanEntityKind> {
    extract_plan_entities(PlanExtractRequest {
        note_id: Some("plan-test".to_string()),
        plan: plan.to_string(),
    })
    .plan_entities
}

#[test]
fn extracts_requested_plan_entities_without_snomed_codes() {
    let response = extract_plan_entities(PlanExtractRequest {
        note_id: Some("plan-1".to_string()),
        plan: "Prescribe amoxicillin. Refer to physiotherapy. Issue eMed3. Review in 2 weeks. \
               Arrange bloods and ECG. BP diary. Dressing change. Medication review. \
               Complete PIP form."
            .to_string(),
    });

    assert_eq!(response.note_id.as_deref(), Some("plan-1"));
    assert_eq!(
        response.plan_entities,
        vec![
            PlanEntityKind::Prescription,
            PlanEntityKind::Referral,
            PlanEntityKind::Emed3,
            PlanEntityKind::Appointment,
            PlanEntityKind::Investigation,
            PlanEntityKind::Monitoring,
            PlanEntityKind::Procedure,
            PlanEntityKind::MedicationReview,
            PlanEntityKind::AdministrativeTask,
        ]
    );
    assert!(response.matches.iter().all(|item| {
        item.field == SoapField::Plan && !item.matched_text.is_empty() && !item.rule_ids.is_empty()
    }));
}

#[test]
fn avoids_removed_or_out_of_scope_plan_entities() {
    let entities = extract(
        "Safety netting discussed. Given lifestyle advice. \
         Social prescribing link worker. Flu jab. Cervical screening recall. \
         Watchful waiting. Text patient with results.",
    );

    assert!(entities.is_empty());
}

#[test]
fn does_not_treat_non_prescription_treatments_as_prescriptions() {
    let entities = extract("Start counselling. Start physio. Start exercise programme.");

    assert!(entities.is_empty());
}

#[test]
fn appointment_requires_definite_review_not_safety_netting() {
    assert_eq!(
        extract("Review in 2 weeks."),
        vec![PlanEntityKind::Appointment]
    );
    assert!(extract("Review if worsening.").is_empty());
}

#[test]
fn detects_emed3_from_fitness_for_work_wording() {
    assert_eq!(
        extract("Patient not fit for work for 2 weeks and should stay off."),
        vec![PlanEntityKind::Emed3]
    );
}

#[test]
fn blocks_conditional_or_declined_actions() {
    let entities = extract(
        "Consider dermatology referral if rash persists. Patient declined antibiotics. \
         No fit note needed.",
    );

    assert!(entities.is_empty());
}
