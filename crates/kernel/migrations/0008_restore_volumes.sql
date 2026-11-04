-- Private restore allocations never appear as public volumes before verification.
ALTER TABLE volume_allocations DROP CONSTRAINT volume_allocations_state_check;
ALTER TABLE volume_allocations ADD CONSTRAINT volume_allocations_state_check CHECK(state IN ('creating','ready','resizing','deleting','deleted','quarantined','restoring','restored'));
ALTER TABLE backup_job_volumes DROP CONSTRAINT backup_job_volumes_state_check;
ALTER TABLE backup_job_volumes ADD CONSTRAINT backup_job_volumes_state_check CHECK(state IN ('pending','uploading','prepared','verified','failed','restoring','restored'));
ALTER TABLE backup_job_volumes ADD CONSTRAINT restored_volume_allocation_fk FOREIGN KEY(restored_volume_id) REFERENCES volume_allocations(volume_id);
