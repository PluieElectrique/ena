-- This table is used used by Sphinx in FoolFuuka

CREATE TABLE IF NOT EXISTS `index_counters` (
  `id` varchar(50) NOT NULL,
  `val` int(10) NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8;
