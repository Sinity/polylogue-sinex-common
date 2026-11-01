//! Dense symmetric positive definite algebra for one component's information matrix.

/// Diagonal of the inverse of the symmetric positive definite `matrix`
/// (row-major, `size` by `size`), by Cholesky factorization.
///
/// With `H = L Lᵀ`, the `i`-th diagonal of `H⁻¹` is the squared norm of the
/// `i`-th column of `L⁻¹`, which is found by one forward substitution per
/// column. Returns `None` when a pivot is not positive, which means the matrix
/// is not positive definite.
pub(crate) fn inverse_diagonal(matrix: &[f64], size: usize) -> Option<Vec<f64>> {
    let mut lower = vec![0.0f64; size * size];
    for column in 0..size {
        let mut pivot = matrix[column * size + column];
        for k in 0..column {
            pivot -= lower[column * size + k] * lower[column * size + k];
        }
        if !pivot.is_finite() || pivot <= 0.0 {
            return None;
        }
        let diagonal = pivot.sqrt();
        lower[column * size + column] = diagonal;
        for row in column + 1..size {
            let mut value = matrix[row * size + column];
            for k in 0..column {
                value -= lower[row * size + k] * lower[column * size + k];
            }
            lower[row * size + column] = value / diagonal;
        }
    }
    let mut diagonal_of_inverse = vec![0.0f64; size];
    let mut solution = vec![0.0f64; size];
    for (index, entry) in diagonal_of_inverse.iter_mut().enumerate() {
        solution.fill(0.0);
        for row in index..size {
            let mut value = if row == index { 1.0 } else { 0.0 };
            for k in index..row {
                value -= lower[row * size + k] * solution[k];
            }
            solution[row] = value / lower[row * size + row];
            *entry += solution[row] * solution[row];
        }
    }
    Some(diagonal_of_inverse)
}
