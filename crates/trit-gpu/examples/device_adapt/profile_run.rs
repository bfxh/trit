//! 消费实测档案并重训策略：合成样本 + 实测档案样本（实测标签）。
//!
//! 真机评估是 **in-sample**（实测形状全部用于重训）；合成留出设备上的评估
//! 仍是无泄漏的。诚实口径写在输出 `note` 与文档里。

use serde_json::json;

use crate::device::Device;
use crate::measured::MeasuredProfile;
use crate::plan::{Plan, Shape, oracle};
use crate::policy::{Fixed, Heuristic, Learned, Policy, RandomChooser, Sample, features};
use crate::report::{
    MeasuredOracle, best_of, evaluate, evaluate_measured, print_eval, print_shape_choice,
};

pub struct Inputs<'a> {
    pub profile: &'a MeasuredProfile,
    pub plans: &'a [Plan],
    pub fixed_plans: [usize; 3],
    pub train_shapes: &'a [Shape],
    pub test_devices: &'a [Device],
    pub clean_samples: &'a [Sample],
    pub clean_policy: &'a mut Policy,
    pub seed: u64,
    pub epochs: usize,
    pub hidden: usize,
    pub repeats: usize,
}

pub fn run(inputs: Inputs<'_>) {
    let plans = inputs.plans;
    let measured = inputs.profile.measured(plans);
    let mut samples = inputs.clean_samples.to_vec();
    let mut real_samples = 0usize;
    if measured.is_empty() {
        // 没有实测形状：把档案设备本身作为一条合成训练设备（模型标签）。
        for &shape in inputs.train_shapes {
            let (label, _) = oracle(plans, &inputs.profile.device, shape);
            for _ in 0..inputs.repeats {
                samples.push(Sample {
                    features: features(&inputs.profile.device, shape),
                    label,
                });
                real_samples += 1;
            }
        }
    } else {
        for (shape, latencies) in &measured {
            let (label, _) = best_of(latencies);
            for _ in 0..inputs.repeats {
                samples.push(Sample {
                    features: features(&inputs.profile.device, *shape),
                    label,
                });
                real_samples += 1;
            }
        }
    }
    let (mut retrained, accuracy) =
        crate::policy::train(&samples, inputs.epochs, inputs.hidden, inputs.seed ^ 0x300);
    println!(
        "{}",
        json!({
            "stage": "train",
            "policy": "learned_profile",
            "samples": samples.len(),
            "profile_samples": real_samples,
            "repeats": inputs.repeats,
            "epochs": inputs.epochs,
            "accuracy": (accuracy * 1000.0).round() / 1000.0,
            "profile_device": inputs.profile.device.name,
            "measured_shapes": measured.len(),
        })
    );

    // 合成留出设备：clean vs retrained（模型评估，无真机泄漏）。
    let shapes = test_shapes();
    {
        let mut chooser = Learned {
            policy: inputs.clean_policy,
        };
        print_eval(
            "learned_clean",
            "synthetic",
            &evaluate(&mut chooser, inputs.test_devices, &shapes, plans),
        );
    }
    {
        let mut chooser = Learned {
            policy: &mut retrained,
        };
        print_eval(
            "learned_profile",
            "synthetic",
            &evaluate(&mut chooser, inputs.test_devices, &shapes, plans),
        );
    }

    if measured.is_empty() {
        return;
    }
    println!(
        "{}",
        json!({"stage": "note", "text": "real measured eval is in-sample: all measured shapes were used for retraining"})
    );
    let mut chooser = MeasuredOracle {
        measured: &measured,
    };
    print_eval(
        "oracle_measured",
        "measured",
        &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
    );
    let mut chooser = Heuristic;
    print_eval(
        "heuristic",
        "measured",
        &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
    );
    let mut chooser = Fixed {
        per_class: inputs.fixed_plans,
    };
    print_eval(
        "class_fixed",
        "measured",
        &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
    );
    let mut chooser = RandomChooser::new(inputs.seed ^ 0x5151);
    print_eval(
        "random",
        "measured",
        &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
    );
    {
        let mut chooser = Learned {
            policy: inputs.clean_policy,
        };
        print_eval(
            "learned_clean",
            "measured",
            &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
        );
    }
    {
        let mut chooser = Learned {
            policy: &mut retrained,
        };
        print_eval(
            "learned_profile",
            "measured",
            &evaluate_measured(&mut chooser, &inputs.profile.device, plans, &measured),
        );
    }
    print_shape_choice("learned_clean", plans, &measured, |shape| {
        inputs
            .clean_policy
            .choose(&inputs.profile.device, shape, plans)
    });
    print_shape_choice("learned_profile", plans, &measured, |shape| {
        retrained.choose(&inputs.profile.device, shape, plans)
    });
}

// 保持 `device_adapt` 的测试形状与主程序一致（main 里是私有常量）。
pub fn test_shapes() -> [Shape; 8] {
    [
        Shape::new(96, 96, 96),
        Shape::new(192, 192, 192),
        Shape::new(320, 320, 320),
        Shape::new(768, 768, 768),
        Shape::new(1536, 1536, 1536),
        Shape::new(64, 2048, 2048),
        Shape::new(2048, 2048, 64),
        Shape::new(4096, 4096, 4096),
    ]
}
